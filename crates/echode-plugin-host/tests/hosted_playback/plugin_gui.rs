//! The plugin's own window and the changes a plugin makes on its own, through
//! the product engine and a real helper. `ECHODE_TEST_FAULT=gui:<socket>`
//! gives the helper a recorded window instead of AppKit, and lets the test
//! act in the test plugin's window like a user (`crates/echode-test-plugin/src/gui.rs`).

use super::device_params::{close, hear, params_route, stereo_clip};
use super::*;
use echode_engine::{ParamEvent, ParamEventKind, PluginActivity};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};

struct Window {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Window {
    /// The plugin connects when the host creates its view.
    fn accept(listener: &UnixListener) -> Self {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "the plugin view never connected");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("{error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
        }
    }

    fn line(&mut self) -> String {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        line.trim().to_string()
    }

    fn lines(&mut self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.line()).collect()
    }

    /// One user action in the plugin's window, written once the plugin has
    /// reported it to the host.
    fn act(&mut self, command: &str) {
        self.writer
            .write_all(format!("{command}\n").as_bytes())
            .unwrap();
        assert_eq!(self.line(), "ok", "{command}");
    }
}

fn gui_session(
    name: &str,
) -> (
    UnixListener,
    std::path::PathBuf,
    echode_engine::PlaybackControl,
    echode_engine::PlaybackAudio,
    Id,
    usize,
) {
    let (project, track) = Project::new().add_track("window");
    let device = Id::from_raw(0x6A);
    let project = project
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 64,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap()
        .0;
    // A short name: Unix socket paths have a small fixed limit on macOS.
    let socket = std::env::temp_dir().join(format!("echode-{name}-{}", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    let mut route = params_route(None);
    route.fault = Some(format!("gui:{}", socket.display()));
    let frames = 64;
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, frames, &host_bin(), &[route]).unwrap();
    control.set_clip_loader(stereo_clip(track.raw()));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    close(hear(&mut control, &mut audio, frames), 0.25);
    // Stopped; control requests still work without further audio callbacks.
    control.command(PlaybackCommand::Stop).unwrap();
    let _ = hear(&mut control, &mut audio, frames);
    (listener, socket, control, audio, device, frames)
}

/// Polls like the document worker until `done` holds for this device's activity.
fn collect(
    control: &mut echode_engine::PlaybackControl,
    device: Id,
    done: impl Fn(&PluginActivity) -> bool,
) -> PluginActivity {
    let mut seen = PluginActivity::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&seen) {
        assert!(Instant::now() < deadline, "activity so far: {seen:?}");
        control.service();
        for (id, activity) in control.take_plugin_activity() {
            assert_eq!(id, device.raw());
            seen.merge(activity);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    seen
}

fn event(id: u32, kind: ParamEventKind) -> ParamEvent {
    ParamEvent { id, kind }
}

/// Rebuilding the loop's event storage must keep the live helper, including
/// its window, unsaved parameters and values omitted from the plugin state.
#[test]
fn contract_loop_changes_keep_the_open_plugin_and_its_live_state() {
    let (listener, socket, mut control, mut audio, device, frames) = gui_session("gui-loop");
    let endpoint = control.device_control_handle(device.raw()).unwrap();
    endpoint.show_gui("window — Params").unwrap();
    let mut window = Window::accept(&listener);
    assert_eq!(window.lines(3), ["create", "parent", "show"]);
    window.act("value 1 1.5");
    let captured = endpoint.capture_state().unwrap();
    window.act("value 1 1.75");
    window.act("value 3 0.75");
    let initial_events: usize = control.plugin_event_caps().iter().sum();
    for playing in [false, true] {
        if playing {
            control.command(PlaybackCommand::Play).unwrap();
        }
        // This fixture's short loop needs more events than its original bank,
        // so the retained helper also adopts a larger shared mapping.
        for (command, looped) in [
            (PlaybackCommand::SetLoop { start: 0, end: 17 }, true),
            (PlaybackCommand::ClearLoop, false),
        ] {
            control.command(command).unwrap();
            let _ = hear(&mut control, &mut audio, frames);
            if looped {
                assert!(control.plugin_event_caps().iter().sum::<usize>() > initial_events);
            }
            let current = control.device_control_handle(device.raw()).unwrap();
            let report = current.show_gui("window — Params").unwrap();
            assert!(!report.created, "loop changes must keep the plugin window");
            assert_eq!(window.line(), "show");
            let values = current.parameters().unwrap();
            assert_eq!(values[0].value, 1.75, "an unsaved change was lost");
            assert_eq!(values[2].value, 0.75, "the helper was replaced or reset");
            assert_eq!(
                current.applied_state().map(|state| state.bytes().to_vec()),
                Some(captured.clone())
            );
        }
    }
    endpoint.close_gui().unwrap();
    drop((audio, control));
    let _ = std::fs::remove_file(socket);
}

#[test]
fn restarting_a_helper_closes_its_window_lifetime_without_reopening_it() {
    let socket = std::env::temp_dir().join(format!("echode-gui-restart-{}", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    let fault = format!("gui:{}", socket.display());
    let mut host = PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(plugin_dylib().as_path()),
        plugin_id: Some("dev.echode.test-params"),
        fault: Some(&fault),
        workgroup: None,
        fan_in: None,
        // This layout is only the fixture's process buffer, not a product limit.
        layout: SessionLayout {
            max_frames: 64,
            input_channels: 2,
            output_channels: 2,
            input_events: 4,
            output_events: 1,
        },
        sample_rate: 48_000,
    })
    .unwrap();
    let endpoint = host.control_handle();
    endpoint.show_gui("window — Params").unwrap();
    let mut window = Window::accept(&listener);
    assert_eq!(window.lines(3), ["create", "parent", "show"]);
    assert_eq!(endpoint.take_activity().unwrap().window_state, Some(true));
    host.restart().unwrap();
    assert_eq!(endpoint.take_activity().unwrap().window_state, Some(false));
    assert!(endpoint.take_activity().is_none());
    host.shutdown().unwrap();
    let _ = std::fs::remove_file(socket);
}

/// Showing creates the helper's window and the plugin's view, raising an open
/// window does not create another, and neither takes keyboard focus. Closing
/// destroys the view; the plugin closing its window is reported once; the
/// next show creates the window again.
#[test]
fn contract_plugin_window_opens_and_raises_without_focus() {
    let (listener, socket, mut control, _audio, device, _) = gui_session("gui-open");
    let endpoint = control.device_control_handle(device.raw()).unwrap();

    let opened = endpoint.show_gui("window — Params").unwrap();
    assert!(opened.created && opened.visible, "{opened:?}");
    assert!(!opened.key, "a shown plugin window must not take focus");
    let mut window = Window::accept(&listener);
    assert_eq!(window.lines(3), ["create", "parent", "show"]);
    let activity = collect(&mut control, device, |seen| seen.window_state.is_some());
    assert_eq!(activity.window_state, Some(true));

    let raised = endpoint.show_gui("window — Params").unwrap();
    assert!(
        !raised.created && raised.visible && !raised.key,
        "{raised:?}"
    );
    assert_eq!(window.line(), "show");

    endpoint.close_gui().unwrap();
    assert_eq!(window.lines(2), ["hide", "destroy"]);
    // Closing what is closed is not an error.
    endpoint.close_gui().unwrap();

    let activity = collect(&mut control, device, |seen| seen.window_state.is_some());
    assert_eq!(activity.window_state, Some(false));
    let reopened = endpoint.show_gui("window — Params").unwrap();
    assert!(reopened.created && !reopened.key, "{reopened:?}");
    let mut window = Window::accept(&listener);
    assert_eq!(window.lines(3), ["create", "parent", "show"]);

    // The plugin closes its window itself. The parent hears it once.
    window.act("closed");
    let closed = collect(&mut control, device, |seen| seen.closed);
    assert_eq!(closed.window_state, Some(false));
    assert!(closed.relay.is_empty() && !closed.dirty);
    assert_eq!(window.lines(2), ["hide", "destroy"]);
    assert!(control.take_plugin_activity().is_empty());

    // A plugin without a window is refused with a reason.
    let mut host = spawn_declared("dev.echode.test-gain").expect("gain plugin");
    let refused = host.control_handle().show_gui("gain").unwrap_err();
    assert!(refused.to_string().contains("no window"), "{refused}");
    host.shutdown().unwrap();
    let _ = std::fs::remove_file(&socket);
}

/// Values and gestures made in the plugin's window reach the parent in order,
/// with the values read back, whether the transport plays or not. Read-only
/// output is not relayed, and the state marked dirty while the window is
/// open is reported for history.
#[test]
fn contract_plugin_window_relays_gestures_and_values() {
    let (listener, socket, mut control, mut audio, device, frames) = gui_session("gui-relay");
    let endpoint = control.device_control_handle(device.raw()).unwrap();
    endpoint.show_gui("window — Params").unwrap();
    let mut window = Window::accept(&listener);
    assert_eq!(window.lines(3), ["create", "parent", "show"]);

    // Stopped: the plugin's `request_flush` wakes the idle processing thread.
    window.act("begin 1");
    window.act("value 1 1.5");
    window.act("end 1");
    let gesture = collect(&mut control, device, |seen| {
        seen.relay
            .events
            .iter()
            .any(|event| event.kind == ParamEventKind::End)
    });
    assert_eq!(
        gesture.relay.events,
        [
            event(1, ParamEventKind::Begin),
            event(1, ParamEventKind::Value),
            event(1, ParamEventKind::End)
        ]
    );
    assert_eq!(gesture.relay.values.len(), 1);
    assert_eq!(gesture.relay.values[0].value, 1.5);
    assert_eq!(gesture.relay.values[0].text, "1.50x");
    assert!(!gesture.relay.lost);
    assert!(
        gesture.window_open,
        "the relay is tagged with the open window"
    );

    // The change is the plugin's own state, heard and captured like any edit.
    control.command(PlaybackCommand::Play).unwrap();
    close(hear(&mut control, &mut audio, frames), 0.375);
    let mut edited = Vec::from(1.5f64.to_le_bytes());
    edited.extend(0.0f64.to_le_bytes());
    assert_eq!(endpoint.capture_state().unwrap(), edited);

    // Playing: a change without a gesture. The read-only meter before it is
    // output, not an edit, and is left out.
    window.act("value 4 0.5");
    window.act("value 2 1");
    let plain = collect(&mut control, device, |seen| !seen.relay.events.is_empty());
    assert_eq!(plain.relay.events, [event(2, ParamEventKind::Value)]);
    assert_eq!(plain.relay.values[0].text, "Half");
    close(hear(&mut control, &mut audio, frames), 0.1875);

    // A state change without a parameter, while the window is open.
    window.act("dirty");
    let dirty = collect(&mut control, device, |seen| seen.dirty);
    assert!(dirty.relay.events.is_empty());

    endpoint.close_gui().unwrap();
    assert_eq!(window.lines(2), ["hide", "destroy"]);
    let _ = std::fs::remove_file(&socket);
}

/// The helper as `gui_session` makes it, with the project and route kept for a later replace.
fn audit_session(
    name: &str,
) -> (
    UnixListener,
    std::path::PathBuf,
    echode_engine::PlaybackControl,
    echode_engine::PlaybackAudio,
    Id,
    usize,
    Project,
    PluginRoute,
) {
    let (project, track) = Project::new().add_track("window");
    let device = Id::from_raw(0x6A);
    let project = project
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    let socket = std::env::temp_dir().join(format!("echode-{name}-{}", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    let mut route = params_route(None);
    route.fault = Some(format!("gui:{}", socket.display()));
    let frames = 64;
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&route),
    )
    .unwrap();
    control.set_clip_loader(stereo_clip(track.raw()));
    control.replace(&project).unwrap();
    let _ = hear(&mut control, &mut audio, frames);
    (
        listener, socket, control, audio, device, frames, project, route,
    )
}

/// DOC-A1: a change the plugin marks dirty while its window is open is the
/// user's edit (ADR 0132), so the activity must say the window was open.
#[test]
fn audit_doc_a1_a_dirty_change_in_an_open_window_counts_as_a_window_edit() {
    let (listener, socket, mut control, audio, device, _, _, _) = audit_session("audit-a1");
    let endpoint = control.device_control_handle(device.raw()).unwrap();
    endpoint.show_gui("window — Params").unwrap();
    let mut window = Window::accept(&listener);
    assert_eq!(window.lines(3), ["create", "parent", "show"]);
    let _ = collect(&mut control, device, |seen| seen.window_state == Some(true));
    window.act("dirty");
    let seen = collect(&mut control, device, |seen| seen.dirty);
    assert!(
        seen.window_open,
        "a dirty change in an open window reported no open window: {seen:?}"
    );
    endpoint.close_gui().unwrap();
    drop((audio, control));
    let _ = std::fs::remove_file(socket);
}

/// DOC-A2, the real helper's half (ADR 0229). The document worker keeps the
/// held save and names the device live; the session then routes the helper's
/// own bytes instead of the model's older state, so the replacement keeps the
/// helper, its window and the change. The worker's half is
/// `saves::tests::an_unrelated_edit_keeps_a_held_save_and_its_edit`, the
/// session's `a_live_plugin_edit_survives_only_updates_that_keep_its_recorded_state`.
#[test]
fn audit_doc_a2_an_unrelated_edit_keeps_the_held_plugin_change() {
    let (listener, socket, mut control, mut audio, device, frames, project, route) =
        audit_session("audit-a2");
    let endpoint = control.device_control_handle(device.raw()).unwrap();
    endpoint.show_gui("window — Params").unwrap();
    let mut window = Window::accept(&listener);
    assert_eq!(window.lines(3), ["create", "parent", "show"]);
    window.act("value 1 1.5");
    window.act("dirty");
    let started = Instant::now();
    let held = loop {
        control.service();
        if let Some((_, bytes)) = control.take_plugin_saves().into_iter().next() {
            break bytes;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "no background save"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let (edited, _) = project.add_track("unrelated edit");
    let mut live = route.clone();
    live.state = control
        .device_applied_state(device.raw())
        .unwrap()
        .as_ref()
        .map(echode_engine::PluginState::held);
    control.set_routes(&host_bin(), std::slice::from_ref(&live));
    control.replace(&edited).unwrap();
    let adopted = Instant::now();
    loop {
        let _ = hear(&mut control, &mut audio, frames);
        if !control.status().pending_plan {
            break;
        }
        assert!(
            adopted.elapsed() < Duration::from_secs(10),
            "plan never adopted"
        );
    }
    let after = control.device_control_handle(device.raw()).unwrap();
    let report = after.show_gui("window — Params").unwrap();
    assert!(!report.created, "the helper and its window were replaced");
    assert_eq!(
        after.parameters().unwrap()[0].value,
        1.5,
        "the change was lost"
    );
    assert_eq!(
        after.applied_state().map(|state| state.bytes().to_vec()),
        Some(held)
    );
    drop((audio, control));
    let _ = std::fs::remove_file(socket);
}
