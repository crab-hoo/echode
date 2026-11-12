//! Recorded plugin states are compared by name and read only by the helper
//! that loads them (ADR 0329).
use super::*;
use echode_engine::{PluginState, StateReadError, StateReader};
use std::collections::HashMap;
use std::thread::ThreadId;

type Reads = Arc<Mutex<Vec<(String, ThreadId)>>>;

/// A store of `states` that records each read and the thread that made it.
fn recording_reader(states: &[&[u8]], reads: &Reads) -> StateReader {
    let store: HashMap<String, Vec<u8>> = states
        .iter()
        .map(|bytes| (echode_model::content_hash(bytes), bytes.to_vec()))
        .collect();
    let reads = Arc::clone(reads);
    Arc::new(move |name| {
        reads
            .lock()
            .unwrap()
            .push((name.to_owned(), std::thread::current().id()));
        store.get(name).cloned().ok_or_else(|| {
            Arc::new(std::io::Error::from(std::io::ErrorKind::NotFound)) as StateReadError
        })
    })
}

fn params_state(first: f64) -> Vec<u8> {
    let mut bytes = Vec::from(first.to_le_bytes());
    bytes.extend(0.0f64.to_le_bytes());
    bytes
}

fn stored_route(bytes: &[u8], reader: &StateReader) -> PluginRoute {
    let mut route = device_params::params_route(None);
    route.state = Some(PluginState::stored(
        echode_model::content_hash(bytes),
        Arc::clone(reader),
    ));
    route
}

fn settle(
    control: &mut echode_engine::PlaybackControl,
    audio: &mut echode_engine::PlaybackAudio,
    frames: usize,
) {
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    // A hung-preparation guard, not a latency threshold.
    let deadline = Instant::now() + Duration::from_secs(10);
    while control.status().pending_plan {
        control.service();
        assert!(control.take_preparation_error().is_none());
        render_content(audio, &mut left, &mut right);
        assert!(Instant::now() < deadline, "replacement never applied");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// An edit that keeps a helper reads none of its recorded state, so its
/// cost does not grow with the saved bytes. A changed state is read once by
/// the helper that loads it, on the preparation thread.
#[test]
fn contract_recorded_states_are_read_only_by_the_helper_that_loads_them() {
    // The block length bounds this fixture only.
    let frames = 64;
    let (base, track) = Project::new().add_track("parameters");
    let device = Id::generate();
    let base = base
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    let (defaults, edited) = (params_state(1.0), params_state(1.5));
    let reads: Reads = Arc::default();
    let reader = recording_reader(&[&defaults, &edited], &reads);
    let (mut control, mut audio) = playback_with_plugins(
        &base,
        48_000,
        frames,
        &host_bin(),
        &[stored_route(&defaults, &reader)],
    )
    .unwrap();
    assert_eq!(
        reads.lock().unwrap().len(),
        1,
        "the first helper loads its state"
    );
    let original = control.device_control_handle(device.raw()).unwrap();
    control.prepare_in_background();

    let (unrelated, _) = base.add_track("unrelated");
    control.set_routes(&host_bin(), &[stored_route(&defaults, &reader)]);
    control.replace(&unrelated).unwrap();
    settle(&mut control, &mut audio, frames);
    assert_eq!(
        reads.lock().unwrap().len(),
        1,
        "an edit that kept the helper read its recorded state"
    );
    assert!(
        control
            .device_control_handle(device.raw())
            .unwrap()
            .applied_state()
            .zip(original.applied_state())
            .is_some_and(|(now, before)| now == before),
        "the helper was replaced"
    );

    let changed = unrelated
        .set_device_state(device, Some(echode_model::content_hash(&edited)))
        .unwrap();
    control.set_routes(&host_bin(), &[stored_route(&edited, &reader)]);
    control.replace(&changed).unwrap();
    settle(&mut control, &mut audio, frames);
    let reads = reads.lock().unwrap().clone();
    assert_eq!(
        reads.len(),
        2,
        "the changed state was not read exactly once"
    );
    assert_eq!(reads[1].0, echode_model::content_hash(&edited));
    assert_ne!(
        reads[1].1,
        std::thread::current().id(),
        "the control turn read the state"
    );
    let parameters = control
        .device_control_handle(device.raw())
        .unwrap()
        .parameters()
        .unwrap();
    assert_eq!(parameters[0].value, 1.5);
}

/// A recorded state that cannot be read stops only the helper that needed
/// it, with the store's error as the cause.
#[test]
fn an_unreadable_recorded_state_names_its_device_and_cause() {
    let frames = 64;
    let (project, track) = Project::new().add_track("parameters");
    let device = Id::generate();
    let project = project
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    let reads: Reads = Arc::default();
    let reader = recording_reader(&[], &reads);
    let error = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        &[stored_route(&params_state(1.0), &reader)],
    )
    .err()
    .expect("a helper started without its recorded state");
    let echode_engine::PlaybackError::State {
        device: failed,
        error,
    } = error
    else {
        panic!("not a state error: {error:?}");
    };
    assert_eq!(failed, device.raw());
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::NotFound)
    );
}
