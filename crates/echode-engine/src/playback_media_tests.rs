//! Media for a stopped replacement loads away from the control turn (ADR 0147).
use super::*;
use echode_model::{AudioRef, Id, Tick};
use std::sync::atomic::AtomicUsize;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Holds the loader while held, failing after five seconds so a load on the
/// control turn ends the test instead of hanging it.
#[derive(Default)]
struct Gate {
    held: Mutex<bool>,
    changed: Condvar,
    calls: AtomicUsize,
    inside: AtomicUsize,
    overlapped: AtomicBool,
    fail: AtomicBool,
}

impl Gate {
    fn hold(&self) {
        *self.held.lock().unwrap() = true;
    }

    fn release(&self) {
        *self.held.lock().unwrap() = false;
        self.changed.notify_all();
    }

    fn pass(&self) -> Result<(), PlaybackError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut held = self.held.lock().unwrap();
        while *held {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(PlaybackError::Media);
            }
            held = self.changed.wait_timeout(held, left).unwrap().0;
        }
        Ok(())
    }

    fn until_calls(&self, calls: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.calls.load(Ordering::Acquire) < calls {
            assert!(Instant::now() < deadline, "the loader was not called");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Every audio placement sounds a constant 0.25 at its gain for one second
/// from the project start. The length bounds this fixture only.
fn gated_loader(gate: Arc<Gate>) -> ClipLoader {
    Arc::new(move |project, rate| {
        gate.calls.fetch_add(1, Ordering::AcqRel);
        if gate.inside.fetch_add(1, Ordering::AcqRel) > 0 {
            gate.overlapped.store(true, Ordering::Release);
        }
        let passed = gate.pass();
        gate.inside.fetch_sub(1, Ordering::AcqRel);
        passed?;
        if gate.fail.load(Ordering::Acquire) {
            return Err(PlaybackError::Media);
        }
        Ok(project
            .tracks()
            .iter()
            .flat_map(|track| {
                track
                    .clips
                    .iter()
                    .filter(|clip| {
                        project
                            .content(clip.content)
                            .is_some_and(|content| content.audio.is_some())
                    })
                    .map(move |clip| LoadedClip {
                        track: track.id.raw(),
                        start_sample: 0,
                        samples: vec![0.25; rate as usize].into(),
                        right: None,
                        window: None,
                        gain: clip.gain,
                        fade_in: 0,
                        fade_out: 0,
                    })
            })
            .collect())
    })
}

fn with_audio(project: &Project, track: Id, clip: u128) -> Project {
    project
        .place_audio(
            track,
            Id::from_raw(clip),
            Id::from_raw(clip + 1),
            Tick::new(0),
            AudioRef::try_new("d".repeat(64), 48_000, 1, 48_000).unwrap(),
        )
        .unwrap()
}

fn peak(audio: &mut PlaybackAudio, blocks: usize) -> f32 {
    let mut left = [0.0; 64];
    let mut right = left;
    let mut peak = 0.0f32;
    for _ in 0..blocks {
        audio.render(&mut left, &mut right);
        peak = left
            .iter()
            .fold(peak, |peak, sample| peak.max(sample.abs()));
    }
    peak
}

#[test]
fn stopped_replacements_load_new_media_off_the_control_turn() {
    let (base, track) = Project::new().add_track("audio");
    let placed = with_audio(&base, track, 0x51);
    // The block size bounds this fixture only.
    let (mut control, mut audio) = playback(&base, 48_000, 64, PlaybackOptions::default()).unwrap();
    let gate = Arc::new(Gate::default());
    control.set_clip_loader(gated_loader(Arc::clone(&gate)));
    control.replace(&base).unwrap();
    assert_eq!(gate.calls.load(Ordering::Acquire), 1);
    control.load_media_in_background(true);
    gate.hold();
    // Returns while the new placement's media is held on another thread.
    control.replace(&placed).unwrap();
    gate.until_calls(2);
    assert!(control.loading_media() && control.preparing_graph());
    assert!(control.status().pending_plan);
    // A later edit and a loop change join that load; the loader runs once at a time.
    let (edited, _) = placed.add_track("later");
    control.replace(&edited).unwrap();
    control
        .command(PlaybackCommand::SetLoop {
            start: 0,
            end: 4_800,
        })
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_eq!(peak(&mut audio, 4), 0.0, "the previous plan is silent");
    assert_eq!(gate.calls.load(Ordering::Acquire), 2);
    gate.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while control.loading_media() || control.status().pending_plan {
        assert!(
            Instant::now() < deadline,
            "the loaded media was not published"
        );
        control.service();
        assert!(control.take_preparation_error().is_none());
        peak(&mut audio, 1);
        std::thread::sleep(Duration::from_millis(1));
    }
    // About 0.18 per channel after the centre pan.
    assert!(peak(&mut audio, 4) > 0.1);
    assert_eq!(control.meters().len(), 2, "the later edit was kept");
    assert!(!gate.overlapped.load(Ordering::Acquire));
    // A placement already loaded does not start another thread.
    control.replace(&placed).unwrap();
    assert!(!control.loading_media());
}

#[test]
fn a_running_replacement_loads_its_media_on_the_control_turn() {
    let (base, track) = Project::new().add_track("audio");
    let (mut control, _audio) = playback(&base, 48_000, 64, PlaybackOptions::default()).unwrap();
    let gate = Arc::new(Gate::default());
    control.set_clip_loader(gated_loader(Arc::clone(&gate)));
    control.replace(&base).unwrap();
    control.load_media_in_background(false);
    control.replace(&with_audio(&base, track, 0x53)).unwrap();
    assert_eq!(gate.calls.load(Ordering::Acquire), 2);
    assert!(!control.loading_media());
}

#[test]
fn a_failed_background_media_load_is_reported_once_and_releases_the_plan() {
    let (base, track) = Project::new().add_track("audio");
    let (mut control, mut audio) = playback(&base, 48_000, 64, PlaybackOptions::default()).unwrap();
    let gate = Arc::new(Gate::default());
    control.set_clip_loader(gated_loader(Arc::clone(&gate)));
    control.replace(&base).unwrap();
    peak(&mut audio, 1);
    control.load_media_in_background(true);
    gate.fail.store(true, Ordering::Release);
    control.replace(&with_audio(&base, track, 0x55)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let error = loop {
        control.service();
        if let Some(error) = control.take_preparation_error() {
            break error;
        }
        assert!(
            Instant::now() < deadline,
            "the failed load was not reported"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(matches!(error, PlaybackError::Media));
    assert!(!control.loading_media() && !control.preparing_graph());
    assert!(!control.status().pending_plan);
    for _ in 0..4 {
        control.service();
        assert!(control.take_preparation_error().is_none());
    }
    // A later load succeeds and publishes.
    gate.fail.store(false, Ordering::Release);
    control.replace(&with_audio(&base, track, 0x57)).unwrap();
    while control.loading_media() || control.status().pending_plan {
        assert!(Instant::now() < deadline + Duration::from_secs(5));
        control.service();
        assert!(control.take_preparation_error().is_none());
        peak(&mut audio, 1);
        std::thread::sleep(Duration::from_millis(1));
    }
}
