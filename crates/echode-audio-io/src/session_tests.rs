use super::*;
#[test]
fn cancelled_queued_play_does_not_query_hardware_or_open_a_stream() {
    let gate = TransportCancellation::default();
    let play = gate.request(&AudioCommand::Play);
    let stop = gate.request(&AudioCommand::Stop);
    let mut session = Session::new(Arc::new(Project::new()));
    session.cancellation = gate;
    session.perform_requested(AudioCommand::Play, play).unwrap();
    assert!(session.notifications.is_none() && session.stream.is_none());
    assert!(!session.state().playing && !session.state().running);
    session.perform_requested(AudioCommand::Stop, stop).unwrap();
    assert_eq!(session.state().tick, 0);
}
fn device(id: u32, uid: &str) -> OutputDevice {
    OutputDevice {
        id,
        uid: uid.into(),
        name: uid.into(),
        sample_rate: 48000,
        buffer_frames: 128,
        channels: 2,
        formats: Vec::new(),
    }
}
#[test]
fn a_pinned_uid_falls_back_and_returns_with_a_new_hal_id() {
    let mut inventory = Inventory {
        default_device: 1,
        devices: vec![device(1, "speaker"), device(2, "headphone")],
    };
    assert_eq!(desired(&inventory, None).unwrap().id, 1);
    assert_eq!(desired(&inventory, Some("headphone")).unwrap().id, 2);
    inventory.default_device = 2;
    assert_eq!(desired(&inventory, Some("speaker")).unwrap().id, 1);
    inventory.devices.remove(0);
    assert_eq!(desired(&inventory, Some("speaker")).unwrap().id, 2);
    inventory.devices.push(device(99, "speaker"));
    assert_eq!(desired(&inventory, Some("speaker")).unwrap().id, 99);
    inventory.devices.clear();
    assert!(desired(&inventory, Some("speaker")).is_none());
}
#[test]
fn transport_status_stop_and_seek_do_not_query_hardware_or_edit_the_project() {
    let project = Arc::new(Project::new());
    let mut session = Session::new(project.clone());
    session
        .perform(AudioCommand::Locate { tick: 705600 })
        .unwrap();
    session.perform(AudioCommand::Pause).unwrap();
    assert_eq!(session.state().tick, 705600);
    session.perform(AudioCommand::Stop).unwrap();
    assert_eq!(session.state().tick, 705600);
    assert!(session.perform(AudioCommand::Locate { tick: -1 }).is_err());
    session
        .perform(AudioCommand::Metronome { enabled: true })
        .unwrap();
    session
        .perform(AudioCommand::Loop {
            start: Some(0),
            end: Some(705_600),
        })
        .unwrap();
    assert!(session.state().metronome);
    assert_eq!(session.state().loop_start, Some(0));
    assert_eq!(session.state().loop_end, Some(705_600));
    assert!(
        session
            .perform(AudioCommand::Loop {
                start: Some(4),
                end: Some(4),
            })
            .is_err()
    );
    assert_eq!(session.state().loop_end, Some(705_600));
    session
        .perform(AudioCommand::Loop {
            start: None,
            end: None,
        })
        .unwrap();
    assert_eq!(session.state().loop_start, None);
    assert!(session.notifications.is_none());
    assert!(Arc::ptr_eq(&session.project, &project));
}

#[test]
#[ignore = "needs CoreAudio; make spike-audio runs the silent product session"]
fn a_retained_session_resumes_without_preparing_media_again() {
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = loads.clone();
    let mut session = Session::new(Arc::new(Project::new()));
    session.clips = Arc::new(move |_, _| {
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(Vec::new())
    });
    let cold = Instant::now();
    session.perform(AudioCommand::Play).unwrap();
    println!("session cold prepare: {:?}", cold.elapsed());
    let prepared = loads.load(Ordering::Relaxed);
    assert!(prepared > 0);
    for command in [AudioCommand::Pause, AudioCommand::Stop, AudioCommand::Pause] {
        session.perform(command).unwrap();
        let held = session.state().tick;
        let before = session.stream.as_ref().unwrap().callbacks();
        let until = Instant::now() + Duration::from_secs(2);
        while session.stream.as_ref().unwrap().callbacks() <= before {
            assert!(Instant::now() < until, "paused HAL callbacks stopped");
            std::thread::sleep(Duration::from_millis(5));
            session.service();
        }
        assert_eq!(session.state().tick, held);
        assert!(!session.state().playing && !session.state().running);
        let warm = Instant::now();
        session.perform(AudioCommand::Play).unwrap();
        println!("session retained resume: {:?}", warm.elapsed());
        assert_eq!(loads.load(Ordering::Relaxed), prepared);
        let until = Instant::now() + Duration::from_secs(2);
        while session.state().tick <= held {
            assert!(Instant::now() < until, "resume did not advance");
            std::thread::sleep(Duration::from_millis(5));
            session.service();
        }
    }
}

#[test]
#[ignore = "needs CoreAudio; make spike-audio runs the silent product session"]
fn a_live_session_refreshes_edits_seeks_and_closes_without_a_window() {
    use std::time::{Duration, Instant};
    let project = Arc::new(Project::new());
    let mut session = Session::new(project.clone());
    session.perform(AudioCommand::Refresh).unwrap();
    let uid = session.state().active_device.expect("output").uid;
    session
        .perform(AudioCommand::Output {
            uid: Some(uid.clone()),
        })
        .unwrap();
    session.perform(AudioCommand::Play).unwrap();
    let pump = |session: &mut Session| {
        let until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < until {
            session.service();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(session.state().error.is_none());
    };
    pump(&mut session);
    assert!(session.state().running && session.state().tick > 0);
    session.update(Arc::new(project.add_track("silent").0));
    pump(&mut session);
    assert!(session.state().running);
    session.perform(AudioCommand::Pause).unwrap();
    let held = session.state().tick;
    pump(&mut session);
    assert_eq!(session.state().tick, held);
    session
        .perform(AudioCommand::Locate { tick: 705600 })
        .unwrap();
    session.perform(AudioCommand::Play).unwrap();
    pump(&mut session);
    assert!(session.state().running && session.state().tick > 705600);
    assert_eq!(session.state().selected_uid, Some(uid));
    let before_stop = session.state().tick;
    session.perform(AudioCommand::Stop).unwrap();
    assert!(!session.state().playing && session.state().tick >= before_stop);
}

#[test]
fn a_plugin_hash_keeps_the_fader_preview_without_a_stream() {
    let (project, track) = Project::new().add_track("preview");
    let device = echode_model::Id::generate();
    let project = project
        .add_clap_device(track, device, "dev.echode.capture")
        .unwrap();
    let mut session = Session::new(Arc::new(project.clone()));
    session
        .perform(AudioCommand::PreviewFader {
            track,
            gain: 0.5,
            pan: 0.0,
        })
        .unwrap();
    assert!(session.fader_preview.is_some());
    let next = Arc::new(
        project
            .set_device_state(device, Some("ab".repeat(32)))
            .unwrap(),
    );
    session.update(Arc::clone(&next));
    assert!(session.fader_preview.is_some());
    assert!(Arc::ptr_eq(&session.project, &next));
    assert!(session.stream.is_none());
    assert!(session.error.is_none());
}
#[test]
fn device_access_without_a_running_plugin_does_not_open_hardware() {
    let (project, track) = Project::new().add_track("fx");
    let device = echode_model::Id::from_raw(0xD7);
    let offline = project
        .add_clap_device(track, device, "org.example.fx")
        .unwrap()
        .set_device_offline(device, true)
        .unwrap();
    for project in [project, offline] {
        let mut session = Session::new(Arc::new(project));
        let Err(error) = session.device_control(device) else {
            panic!("a project without a running plugin has no device control");
        };
        assert!(error.contains("no running plugin"), "{error}");
        assert!(session.notifications.is_none() && session.stream.is_none());
        assert!(!session.engine_held && !session.state().playing);
    }
}
#[test]
fn a_plugin_state_only_update_without_a_stream_keeps_the_session_idle() {
    let (project, track) = Project::new().add_track("fx");
    let device = echode_model::Id::from_raw(0xD8);
    let project = project
        .add_clap_device(track, device, "org.example.fx")
        .unwrap();
    let recorded = project
        .set_device_state(device, Some("a".repeat(64)))
        .unwrap();
    let mut session = Session::new(Arc::new(project));
    session.update(Arc::new(recorded.clone()));
    assert_eq!(session.project.as_ref(), &recorded);
    assert!(session.stream.is_none() && session.state().error.is_none());
}
