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
        safety_frames: 0,
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
    // Pausing a stopped transport holds nothing.
    assert_eq!(session.state().tick, 705600);
    assert!(!session.state().paused);
    session.perform(AudioCommand::Stop).unwrap();
    assert_eq!(
        session.state().tick,
        0,
        "stop returns to the remembered start"
    );
    session
        .perform(AudioCommand::SetStart { tick: 352_800 })
        .unwrap();
    assert_eq!(
        (session.state().start, session.state().tick),
        (352_800, 352_800)
    );
    session
        .perform(AudioCommand::Locate { tick: 705600 })
        .unwrap();
    assert_eq!(session.state().start, 352_800, "a locate keeps the start");
    session.perform(AudioCommand::Stop).unwrap();
    assert_eq!(session.state().tick, 352_800);
    assert!(matches!(
        session.perform(AudioCommand::Locate { tick: -1 }),
        Err(AudioError::Rejected(Rejected::NegativePosition))
    ));
    assert!(matches!(
        session.perform(AudioCommand::SetStart { tick: -1 }),
        Err(AudioError::Rejected(Rejected::NegativeStart))
    ));
    assert_eq!(session.state().start, 352_800);
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
    assert!(matches!(
        session.perform(AudioCommand::Loop {
            start: Some(4),
            end: Some(4),
        }),
        Err(AudioError::Rejected(Rejected::Loop))
    ));
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
    // Helpers and media are prepared off this thread; service installs them.
    until_opened(&mut session);
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
fn complete_mixer_groups_validate_before_replacing_the_previous_preview() {
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    let mut session = Session::new(Arc::new(project.clone()));
    let command = serde_json::json!({"type": "audio.preview_faders", "values": [
        {"track": first, "gain": 0.5, "pan": -0.25},
        {"track": second, "gain": 0.25, "pan": 0.5}
    ]});
    session
        .perform(serde_json::from_value(command.clone()).unwrap())
        .unwrap();
    let preview = session.fader_preview.clone();
    assert_eq!(preview.as_ref().unwrap().len(), 2);
    for invalid in [
        serde_json::json!([]),
        serde_json::json!([command["values"][0], command["values"][0]]),
        serde_json::json!([command["values"][0], {"track": second, "gain": 5.0, "pan": 0.0}]),
        serde_json::json!([{"track": echode_model::Id::generate(), "gain": 1.0, "pan": 0.0}]),
    ] {
        let input = serde_json::json!({"type": "audio.preview_faders", "values": invalid});
        assert!(matches!(
            session.perform(serde_json::from_value(input).unwrap()),
            Err(AudioError::Rejected(Rejected::MixerPreview))
        ));
        assert_eq!(session.fader_preview, preview);
    }
    assert!(session.stream.is_none() && session.opening.is_none());
    session.perform(AudioCommand::ClearFaderPreview).unwrap();
    assert!(session.fader_preview.is_none());
    session
        .perform(serde_json::from_value(command).unwrap())
        .unwrap();
    session.update(Arc::new(project.add_track("edit").0));
    assert!(session.fader_preview.is_none());
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
        assert!(
            matches!(error, AudioError::Rejected(Rejected::NoRunningPlugin)),
            "{error}"
        );
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

/// Tracks named "audio" sound a constant 0.25 from the project start, about
/// 0.18 per channel after the centre pan. The clip length covers these
/// fixtures only.
fn audio_tracks_sound(session: &mut Session) {
    session.clips = Arc::new(|project, rate| {
        Ok(project
            .tracks()
            .iter()
            .filter(|track| track.name == "audio")
            .map(|track| echode_engine::LoadedClip {
                track: track.id.raw(),
                start_sample: 0,
                samples: vec![0.25; rate as usize * 4].into(),
                right: None,
                window: None,
                gain: 1.0,
                fade_in: 0,
                fade_out: 0,
            })
            .collect())
    });
}

fn until_opened(session: &mut Session) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while session.state().opening {
        assert!(
            std::time::Instant::now() < deadline,
            "the output never opened"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
        session.service();
    }
}

fn until_waiting(output: &crate::HeadlessOutput) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while output.waiting() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "preparation never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Renders `blocks` callbacks of the installed stream and returns the peak.
fn pumped_peak(session: &mut Session, blocks: usize) -> f32 {
    let mut peak = 0.0f32;
    for _ in 0..blocks {
        let stream = session.stream.as_ref().expect("installed output");
        let data = stream.pump(stream.device.buffer_frames as usize);
        peak = data
            .iter()
            .fold(peak, |peak, sample| peak.max(sample.abs()));
        session.service();
    }
    peak
}

/// Pumps blocks until Play sounds and returns their peak. Play waits for the
/// plan carrying the edits so far while its media or helpers are prepared on
/// other threads (ADR 0330, ADR 0333), so a fixed number of blocks can end
/// before that plan reaches the callback.
fn until_running(session: &mut Session) -> f32 {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut peak = 0.0f32;
    while !session.state().running {
        assert!(std::time::Instant::now() < deadline, "play never started");
        assert!(
            session.state().error.is_none(),
            "{:?}",
            session.state().error
        );
        peak = peak.max(pumped_peak(session, 1));
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    peak
}

#[test]
fn group_previews_started_during_output_preparation_apply_together_under_rtsan() {
    let (project, first) = Project::new().add_track("audio");
    let (project, second) = project.add_track("audio");
    let mut session = Session::new(Arc::new(project));
    audio_tracks_sound(&mut session);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    session
        .perform(AudioCommand::PreviewFader {
            track: first,
            gain: 0.0,
            pan: 0.0,
        })
        .unwrap();
    output.hold();
    session.perform(AudioCommand::Play).unwrap();
    until_waiting(&output);
    session
        .perform(AudioCommand::PreviewFaders {
            values: vec![
                FaderValue {
                    track: first,
                    gain: 0.0,
                    pan: 0.0,
                },
                FaderValue {
                    track: second,
                    gain: 0.0,
                    pan: 0.0,
                },
            ],
        })
        .unwrap();
    output.release();
    until_opened(&mut session);
    assert_eq!(pumped_peak(&mut session, 8), 0.0);
    session.perform(AudioCommand::ClearFaderPreview).unwrap();
    assert!(pumped_peak(&mut session, 16) > 0.3);
    assert!(session.state().error.is_none());
}

#[test]
fn contract_a_first_device_view_opens_the_output_without_holding_edits_or_transport_under_rtsan() {
    let (project, fx) = Project::new().add_track("fx");
    let device = echode_model::Id::from_raw(0xDA);
    let project = project
        .add_clap_device(fx, device, "org.example.fx")
        .unwrap();
    let mut session = Session::new(Arc::new(project.clone()));
    audio_tracks_sound(&mut session);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    output.hold();
    assert!(matches!(
        session.device_control(device),
        Ok(DeviceAccess::Opening)
    ));
    until_waiting(&output);
    // The worker's other turns finish while helpers and media are prepared.
    let (edited, _) = project.add_track("audio");
    session.update(Arc::new(edited.clone()));
    let beat = echode_model::TICKS_PER_QUARTER;
    session
        .perform(AudioCommand::Locate { tick: beat })
        .unwrap();
    session
        .perform(AudioCommand::Loop {
            start: Some(0),
            end: Some(beat * 64),
        })
        .unwrap();
    session.perform(AudioCommand::Play).unwrap();
    let state = session.state();
    assert!(state.opening && state.playing && !state.running);
    assert_eq!(state.tick, beat);
    assert!(matches!(
        session.device_control(device),
        Ok(DeviceAccess::Opening)
    ));
    assert_eq!(output.waiting() + output.prepared(), 1, "one opening only");
    assert!(session.stream.is_none());
    output.release();
    until_opened(&mut session);
    assert_eq!(output.prepared(), 1);
    let state = session.state();
    assert!(state.error.is_none(), "{:?}", state.error);
    assert_eq!(state.tick, beat, "the locate made while opening applies");
    assert_eq!(state.loop_start, Some(0));
    // Play waits for the plan with the edits made while opening, whose graph
    // is prepared on another thread (ADR 0333). The track added while opening
    // then sounds, from the located position.
    until_running(&mut session);
    assert!(pumped_peak(&mut session, 8) > 0.1);
    let state = session.state();
    assert!(state.running && !state.opening && state.tick > beat);
    // An unresolvable plugin has no helper; the request reports that once.
    let Err(error) = session.opened_device_control(device) else {
        panic!("org.example.fx is not installed");
    };
    assert!(
        matches!(
            error,
            AudioError::Playback(echode_engine::PlaybackError::NoRunningPlugin)
        ),
        "{error}"
    );
    session.perform(AudioCommand::Stop).unwrap();
    assert!(!session.state().playing);
}

#[test]
fn contract_stop_returns_to_the_remembered_start_and_pause_holds_under_rtsan() {
    let directory =
        std::env::temp_dir().join(format!("echode-session-start-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let package = directory.join("song.echode");
    let (project, _) = Project::new().add_track("audio");
    echode_history::write_package(&package, &project).unwrap();
    let mut session = Session::with_media(Arc::new(project.clone()), &package);
    audio_tracks_sound(&mut session);
    session.use_headless_output(crate::HeadlessOutput::default());
    let beat = echode_model::TICKS_PER_QUARTER;
    session
        .perform(AudioCommand::SetStart { tick: beat })
        .unwrap();
    session.perform(AudioCommand::Play).unwrap();
    until_opened(&mut session);
    assert!(pumped_peak(&mut session, 8) > 0.1);
    // Pause holds the playhead, and play resumes from there.
    session.perform(AudioCommand::Pause).unwrap();
    pumped_peak(&mut session, 8);
    let held = session.state();
    assert!(held.paused && !held.playing, "{held:?}");
    assert!(held.tick > beat);
    session.perform(AudioCommand::Play).unwrap();
    assert!(pumped_peak(&mut session, 8) > 0.1);
    let resumed = session.state();
    assert!(resumed.tick > held.tick && !resumed.paused);
    // Stop returns to the start at once. The media still fades out from
    // where it stopped instead of being cut by a seek.
    session.perform(AudioCommand::Stop).unwrap();
    assert_eq!(session.state().tick, beat);
    assert!(pumped_peak(&mut session, 1) > 0.0, "stop cut the fade");
    pumped_peak(&mut session, 8);
    let stopped = session.state();
    assert_eq!(stopped.tick, beat);
    assert!(!stopped.paused && !stopped.playing && stopped.error.is_none());
    session.perform(AudioCommand::Play).unwrap();
    assert!(pumped_peak(&mut session, 2) > 0.1);
    assert!(session.state().tick > beat);
    // A new start while playing seeks and keeps playing.
    session
        .perform(AudioCommand::SetStart { tick: beat * 2 })
        .unwrap();
    assert_eq!(session.state().tick, beat * 2);
    assert!(pumped_peak(&mut session, 2) > 0.1);
    assert!(session.state().running && session.state().tick > beat * 2);
    session.perform(AudioCommand::Stop).unwrap();
    assert_eq!(session.state().tick, beat * 2);
    // The start is the package's, read when it opens again.
    drop(session);
    let reopened = Session::with_media(Arc::new(project), &package);
    assert_eq!(
        (reopened.state().start, reopened.state().tick),
        (beat * 2, beat * 2)
    );
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn a_stop_while_playback_opens_the_output_installs_it_stopped() {
    let (project, _) = Project::new().add_track("audio");
    let mut session = Session::new(Arc::new(project));
    audio_tracks_sound(&mut session);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    output.hold();
    session.perform(AudioCommand::Play).unwrap();
    assert!(session.state().opening && session.state().playing);
    until_waiting(&output);
    session.perform(AudioCommand::Stop).unwrap();
    assert!(!session.state().playing);
    output.release();
    until_opened(&mut session);
    assert_eq!(
        pumped_peak(&mut session, 8),
        0.0,
        "a stopped Play stays unheard"
    );
    assert_eq!(session.state().tick, 0);
    assert!(!session.state().running);
    // The retained output resumes without preparing again.
    session.perform(AudioCommand::Play).unwrap();
    assert!(pumped_peak(&mut session, 8) > 0.1);
    assert!(session.state().running && session.state().tick > 0);
    assert_eq!(output.prepared(), 1);
}

#[test]
fn a_failed_opening_reports_its_error_and_is_not_retried_by_service() {
    let (project, fx) = Project::new().add_track("fx");
    let device = echode_model::Id::from_raw(0xDB);
    let project = project
        .add_clap_device(fx, device, "org.example.fx")
        .unwrap();
    let mut session = Session::new(Arc::new(project));
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    output.fail_next(AudioError::Playback(echode_engine::PlaybackError::Plugin));
    assert!(matches!(
        session.device_control(device),
        Ok(DeviceAccess::Opening)
    ));
    until_opened(&mut session);
    // The state shows the failure's text; the request keeps its cause.
    assert_eq!(
        session.state().error.as_deref(),
        Some("plugin helper did not start")
    );
    let Err(error) = session.opened_device_control(device) else {
        panic!("the output did not open");
    };
    assert!(
        matches!(
            error,
            AudioError::Playback(echode_engine::PlaybackError::Plugin)
        ),
        "{error}"
    );
    for _ in 0..4 {
        session.service();
    }
    assert!(!session.opening() && session.stream.is_none());
    assert_eq!(output.prepared(), 1);
}

#[test]
fn retained_output_replacements_wait_for_new_devices_without_reopening() {
    let (base, track) = Project::new().add_track("audio");
    let mut session = Session::new(Arc::new(base.clone()));
    audio_tracks_sound(&mut session);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    session.perform(AudioCommand::Play).unwrap();
    until_opened(&mut session);
    assert!(pumped_peak(&mut session, 8) > 0.1);
    // This route has no plugin and can never load this executable. It only
    // selects graph preparation when sanitizer builds have no helper binary.
    session
        .stream
        .as_mut()
        .unwrap()
        .set_test_plugin_routes(&std::env::current_exe().unwrap(), &[]);
    let device = echode_model::Id::generate();
    let changed = base
        .add_clap_device(track, device, "org.example.fx")
        .unwrap();
    session.update(Arc::new(changed));
    assert!(
        session.opening(),
        "replacement preparation was not asynchronous"
    );
    assert!(
        !session.state().opening,
        "an open output is not opening again"
    );
    assert!(
        session.state().updating,
        "the replacement a new device waits for is reported"
    );
    assert!(session.state().pending_plan);
    assert!(matches!(
        session.device_control(device),
        Ok(DeviceAccess::Opening)
    ));
    // Undo and loop changes coalesce while a bank is being prepared.
    session.update(Arc::new(base));
    session
        .perform(AudioCommand::Loop {
            start: Some(0),
            end: Some(705_600),
        })
        .unwrap();
    session.perform(AudioCommand::Stop).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while session.opening() || session.state().pending_plan {
        pumped_peak(&mut session, 1);
        assert!(
            session.state().error.is_none(),
            "{:?}",
            session.state().error
        );
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!session.state().playing && !session.state().updating);
    assert!(session.opened_device_control(device).is_err());
    assert_eq!(output.prepared(), 1);
}

/// Holds the media loader while held. It fails after five seconds, so a load
/// on the test thread, which stands in for the worker, ends instead of hanging.
#[derive(Clone, Default)]
struct MediaGate(Arc<(std::sync::Mutex<bool>, std::sync::Condvar, AtomicU64)>);

impl MediaGate {
    fn hold(&self) {
        *self.0.0.lock().unwrap() = true;
    }

    fn release(&self) {
        *self.0.0.lock().unwrap() = false;
        self.0.1.notify_all();
    }

    fn pass(&self) -> Result<(), echode_engine::PlaybackError> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut held = self.0.0.lock().unwrap();
        self.0.2.fetch_add(1, Ordering::AcqRel);
        while *held {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                self.0.2.fetch_sub(1, Ordering::AcqRel);
                return Err(echode_engine::PlaybackError::Media);
            }
            held = self.0.1.wait_timeout(held, left).unwrap().0;
        }
        self.0.2.fetch_sub(1, Ordering::AcqRel);
        Ok(())
    }

    fn until_waiting(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.0.2.load(Ordering::Acquire) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the media load never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// Audio placements sound a constant 0.25 at their gain from the project
/// start, about 0.18 per channel at gain 1 after the centre pan. The clip
/// length covers these fixtures only.
fn gated_media(session: &mut Session, gate: &MediaGate) {
    let gate = gate.clone();
    session.clips = Arc::new(move |project, rate| {
        gate.pass()?;
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
                    .map(move |clip| echode_engine::LoadedClip {
                        track: track.id.raw(),
                        start_sample: 0,
                        samples: vec![0.25; rate as usize * 4].into(),
                        right: None,
                        window: None,
                        gain: clip.gain,
                        fade_in: 0,
                        fade_out: 0,
                    })
            })
            .collect())
    });
}

#[test]
fn contract_a_prepared_output_plays_at_once_and_loads_stopped_edits_off_the_worker_under_rtsan() {
    let (base, track) = Project::new().add_track("audio");
    let clip = echode_model::Id::from_raw(0x61);
    let project = base
        .place_audio(
            track,
            clip,
            echode_model::Id::from_raw(0x62),
            echode_model::Tick::new(0),
            echode_model::AudioRef::try_new("c".repeat(64), 48_000, 1, 192_000).unwrap(),
        )
        .unwrap();
    let mut session = Session::new(Arc::new(project.clone()));
    let gate = MediaGate::default();
    gated_media(&mut session, &gate);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    // Opening the project prepares helpers, media and the output, stopped.
    session.perform(AudioCommand::Prepare).unwrap();
    let state = session.state();
    assert!(state.opening && !state.playing, "{state:?}");
    until_opened(&mut session);
    assert!(
        session.state().error.is_none(),
        "{:?}",
        session.state().error
    );
    assert_eq!(
        pumped_peak(&mut session, 4),
        0.0,
        "a prepared output is stopped"
    );
    // A stopped edit needing new media returns while that media loads.
    gate.hold();
    let quieter = project.set_clip_gain(clip, 0.5).unwrap();
    session.update(Arc::new(quieter));
    gate.until_waiting();
    let state = session.state();
    assert!(session.opening() && state.pending_plan && !state.opening);
    assert!(state.error.is_none(), "{:?}", state.error);
    // Play waits for it instead of sounding the plan it replaces.
    session.perform(AudioCommand::Play).unwrap();
    let state = session.state();
    assert!(state.playing && !state.running);
    assert_eq!(
        pumped_peak(&mut session, 8),
        0.0,
        "the old plan stayed silent"
    );
    gate.release();
    let peak = until_running(&mut session).max(pumped_peak(&mut session, 8));
    // Half the gain: about 0.09 per channel. The old plan would give 0.18.
    assert!(peak > 0.05 && peak < 0.1, "{peak}");
    assert!(
        session.state().error.is_none(),
        "{:?}",
        session.state().error
    );
    assert_eq!(output.prepared(), 1, "the prepared output was kept");
}

/// Pumps blocks until the newest edit's plan reaches the callback.
fn until_published(session: &mut Session) -> usize {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut blocks = 0;
    while session.opening() || session.state().pending_plan {
        assert!(
            std::time::Instant::now() < deadline,
            "the edit never reached the callback"
        );
        assert!(
            session.state().error.is_none(),
            "{:?}",
            session.state().error
        );
        pumped_peak(session, 1);
        blocks += 1;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    blocks
}

/// ADR 0330: while playing, an edit needing new media returns while it loads
/// and the plan it replaces keeps sounding. A tempo edit's plan keeps the
/// beat when the callback adopts it, and a locate meanwhile lands on its
/// tick in the plan still sounding.
#[test]
fn contract_playing_edits_load_media_off_the_worker_and_keep_the_beat_under_rtsan() {
    let (base, track) = Project::new().add_track("audio");
    let clip = echode_model::Id::from_raw(0x71);
    let project = base
        .place_audio(
            track,
            clip,
            echode_model::Id::from_raw(0x72),
            echode_model::Tick::new(0),
            echode_model::AudioRef::try_new("c".repeat(64), 48_000, 1, 192_000).unwrap(),
        )
        .unwrap();
    let mut session = Session::new(Arc::new(project.clone()));
    let gate = MediaGate::default();
    gated_media(&mut session, &gate);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    session.perform(AudioCommand::Play).unwrap();
    until_opened(&mut session);
    until_running(&mut session);
    // About 0.18 per channel at gain 1 after the centre pan.
    let peak = pumped_peak(&mut session, 8);
    assert!(peak > 0.15, "{peak}");
    // The edit returns while its media is held on another thread.
    gate.hold();
    let quieter = project.set_clip_gain(clip, 0.5).unwrap();
    session.update(Arc::new(quieter.clone()));
    gate.until_waiting();
    let state = session.state();
    assert!(state.running && state.pending_plan, "{state:?}");
    let peak = pumped_peak(&mut session, 8);
    assert!(peak > 0.15, "the replaced plan stopped sounding: {peak}");
    gate.release();
    until_published(&mut session);
    let peak = pumped_peak(&mut session, 8);
    assert!(peak > 0.05 && peak < 0.1, "{peak}");
    // A tempo edit also returns while its media loads. The old plan keeps
    // its tempo until then.
    gate.hold();
    let slower = quieter.with_tempo(
        echode_model::Tick::new(0),
        echode_model::Tempo::quarters_per_minute(60).unwrap(),
    );
    session.update(Arc::new(slower.clone()));
    gate.until_waiting();
    assert!(session.state().running && session.state().pending_plan);
    let quarter = echode_model::TICKS_PER_QUARTER;
    // One 128-frame block at 120 quarters per minute, rounded up.
    let block = quarter * 128 / 24_000 + 1;
    let target = 4 * quarter;
    session
        .perform(AudioCommand::Locate { tick: target })
        .unwrap();
    pumped_peak(&mut session, 1);
    let tick = session.state().tick;
    assert!(
        (target..=target + block).contains(&tick),
        "the locate missed its beat in the sounding plan: {tick}"
    );
    gate.release();
    let blocks = i64::try_from(until_published(&mut session)).unwrap();
    let state = session.state();
    assert!(state.running && state.error.is_none(), "{state:?}");
    assert!(
        (tick..=tick + (blocks + 1) * block).contains(&state.tick),
        "adopting the tempo edit moved the beat: {} after {blocks} blocks from {tick}",
        state.tick
    );
    assert_eq!(output.prepared(), 1, "the output was kept");
}

/// ADR 0330: an edit made while a playing output opened loads its media off
/// the worker at install. The output waits for it, as Play does, instead of
/// sounding the plan it replaces.
#[test]
fn contract_an_install_while_playing_waits_for_the_media_of_an_edit_made_meanwhile_under_rtsan() {
    let (base, track) = Project::new().add_track("audio");
    let clip = echode_model::Id::from_raw(0x81);
    let project = base
        .place_audio(
            track,
            clip,
            echode_model::Id::from_raw(0x82),
            echode_model::Tick::new(0),
            echode_model::AudioRef::try_new("c".repeat(64), 48_000, 1, 192_000).unwrap(),
        )
        .unwrap();
    let mut session = Session::new(Arc::new(project.clone()));
    let gate = MediaGate::default();
    gated_media(&mut session, &gate);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    output.hold();
    session.perform(AudioCommand::Play).unwrap();
    until_waiting(&output);
    session.update(Arc::new(project.set_clip_gain(clip, 0.5).unwrap()));
    output.release();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while output.prepared() == 0 {
        assert!(std::time::Instant::now() < deadline, "never prepared");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    // The opening loaded the project it began with. Installing sends the
    // edit, whose media is held.
    gate.hold();
    until_opened(&mut session);
    gate.until_waiting();
    let state = session.state();
    assert!(state.playing && !state.running, "{state:?}");
    assert_eq!(
        pumped_peak(&mut session, 8),
        0.0,
        "the plan the edit replaced sounded"
    );
    gate.release();
    let peak = until_running(&mut session).max(pumped_peak(&mut session, 8));
    // Half the gain: about 0.09 per channel. The replaced plan gives 0.18.
    assert!(peak > 0.05 && peak < 0.1, "{peak}");
    assert_eq!(output.prepared(), 1);
}

/// ADR 0334: closing a document while its output prepares stops the
/// preparation's helper starts instead of waiting for them. The gate stands
/// in for a held helper start and stays held.
#[test]
fn contract_closing_while_the_output_prepares_cancels_the_preparation() {
    let (project, _) = Project::new().add_track("audio");
    let mut session = Session::new(Arc::new(project));
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    output.hold();
    session.perform(AudioCommand::Play).unwrap();
    until_waiting(&output);
    // A hung-close guard, not a latency threshold: if closing waits, the
    // watchdog releases the gate after it and records that it had to.
    let (closed, watched) = std::sync::mpsc::channel::<()>();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = {
        let output = output.clone();
        let fired = Arc::clone(&fired);
        std::thread::spawn(move || {
            if watched
                .recv_timeout(std::time::Duration::from_secs(10))
                .is_err()
            {
                fired.store(true, Ordering::Release);
                output.release();
            }
        })
    };
    drop(session);
    // The watchdog is gone if it fired.
    let _ = closed.send(());
    watchdog.join().unwrap();
    output.release();
    assert!(
        !fired.load(Ordering::Acquire),
        "closing waited for the held preparation"
    );
    assert_eq!(output.prepared(), 1);
}

/// ADR 0333: an edit made while a playing output opened needs a new graph but
/// no new media, here a second track. Installing prepares that graph on the
/// preparation thread, where helpers it adds start, instead of on the worker.
/// Play waits for its plan instead of sounding the plan it replaces.
#[test]
fn contract_an_install_prepares_the_graph_of_an_edit_made_meanwhile_off_the_worker_under_rtsan() {
    let (base, _) = Project::new().add_track("audio");
    let mut session = Session::new(Arc::new(base.clone()));
    audio_tracks_sound(&mut session);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    output.hold();
    session.perform(AudioCommand::Play).unwrap();
    until_waiting(&output);
    let (edited, _) = base.add_track("audio");
    session.update(Arc::new(edited));
    output.release();
    // Polled here rather than by service, to see what installing leaves.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut prepared = loop {
        if let Some(result) = session.opening.as_mut().and_then(Opening::poll) {
            break result.unwrap();
        }
        assert!(std::time::Instant::now() < deadline, "never prepared");
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let opening = session.opening.take().unwrap();
    // This route has no plugin and can never load this executable. It only
    // selects graph preparation when sanitizer builds have no helper binary.
    prepared.set_test_plugin_routes(&std::env::current_exe().unwrap(), &[]);
    session.install(opening, prepared).unwrap();
    assert!(
        session.stream.as_ref().unwrap().preparing_graph(),
        "installing prepared the edit's graph on the worker"
    );
    let state = session.state();
    assert!(state.playing && !state.running, "{state:?}");
    // Without service the plan cannot be published, so nothing may sound.
    let stream = session.stream.as_ref().unwrap();
    for _ in 0..8 {
        let data = stream.pump(stream.device.buffer_frames as usize);
        assert!(
            data.iter().all(|sample| *sample == 0.0),
            "the plan the edit replaced sounded"
        );
    }
    until_running(&mut session);
    // Past the start's fade: two tracks give about 0.35 per channel, the
    // replaced plan 0.18.
    pumped_peak(&mut session, 8);
    let peak = pumped_peak(&mut session, 8);
    assert!(peak > 0.3 && peak < 0.4, "{peak}");
    assert_eq!(output.prepared(), 1);
}

#[test]
fn prepare_without_a_running_plugin_opens_once_and_a_later_play_reuses_it() {
    let (project, _) = Project::new().add_track("audio");
    let mut session = Session::new(Arc::new(project));
    audio_tracks_sound(&mut session);
    let output = crate::HeadlessOutput::default();
    session.use_headless_output(output.clone());
    output.hold();
    session.perform(AudioCommand::Prepare).unwrap();
    until_waiting(&output);
    // Asking again while it opens does not prepare twice.
    session.perform(AudioCommand::Prepare).unwrap();
    output.release();
    until_opened(&mut session);
    session.perform(AudioCommand::Prepare).unwrap();
    assert_eq!(output.prepared(), 1);
    assert!(!session.state().opening && !session.state().running);
    session.perform(AudioCommand::Play).unwrap();
    assert!(pumped_peak(&mut session, 8) > 0.1);
    assert!(session.state().running);
    assert_eq!(output.prepared(), 1);
}

/// DOC-A2 (ADR 0229): an unrelated edit keeps a helper's unrecorded change;
/// Undo of that device, its removal or taking it offline loads the recorded bytes.
#[test]
fn a_live_plugin_edit_survives_only_updates_that_keep_its_recorded_state() {
    let (project, track) = Project::new().add_track("fx");
    let device = echode_model::Id::from_raw(0x2A);
    let project = project
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    let mut session = Session::new(Arc::new(project.clone()));
    session.keep_live_plugin_edits(vec![device.raw(), 0x2B]);
    let (unrelated, _) = project.add_track("unrelated edit");
    assert_eq!(session.live_edits_kept_by(&unrelated), [device.raw()]);
    let undone = project
        .set_device_state(device, Some("a".repeat(64)))
        .unwrap();
    let offline = project.set_device_offline(device, true).unwrap();
    let removed = project.remove_device(device).unwrap();
    for next in [undone, offline, removed] {
        assert!(session.live_edits_kept_by(&next).is_empty());
    }
}

/// DIA-03 (docs/reviews/2026-11-13-detailed-implementation-audit.md): after a
/// device change whose inventory cannot be read, every service turn reads it
/// again and logs the failure again. The worker services 50 times a second.
#[test]
#[ignore = "DIA-03: a failed inventory read is retried and logged on every service turn"]
fn audit_dia03_a_failed_inventory_read_is_not_retried_on_every_service_turn() {
    struct Unreadable(AtomicU64);
    impl Outputs for Unreadable {
        fn refresh(
            &self,
            _epoch: &Arc<AtomicU64>,
        ) -> Result<(Inventory, Option<hardware::Notifications>), AudioError> {
            if self.0.fetch_add(1, Ordering::AcqRel) == 0 {
                Ok((Inventory::default(), None))
            } else {
                Err(crate::HardwareFault::SampleRate.into())
            }
        }
        fn claim(&self, _device: OutputDevice) -> Result<crate::stream::Claim, AudioError> {
            Err(AudioError::NoOutputDevice)
        }
        fn register(&self, _stream: &mut OutputStream) -> Result<(), AudioError> {
            Ok(())
        }
    }
    let outputs = Arc::new(Unreadable(AtomicU64::new(0)));
    let mut session = Session::new(Arc::new(Project::new()));
    session.outputs = Arc::clone(&outputs) as Arc<dyn Outputs>;
    session.perform(AudioCommand::Refresh).unwrap();
    // A device arrives whose properties cannot be read (IQA-06).
    session.hardware_epoch.fetch_add(1, Ordering::AcqRel);
    for _ in 0..100 {
        session.service();
    }
    assert!(session.state().error.is_some());
    let reads = outputs.0.load(Ordering::Acquire) - 1;
    assert!(
        reads <= 3,
        "100 service turns in a row read the inventory {reads} times"
    );
}
