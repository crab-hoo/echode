use super::{FanIn, PluginControl, inheritable, push_stderr};
use crate::control::{ControlMessage, Socket};
use crate::gui::{GuiReport, ParamEvent, ParamEventKind, ParamRelay};
use std::os::fd::AsRawFd;
use std::time::Duration;

/// Window notices and plugin-made changes leave the inbox, so a request
/// waiting for its reply never takes them, and a dirty notice counts as
/// the user's change only while the window is open.
#[test]
fn window_activity_is_kept_apart_from_replies() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    let answer = std::thread::spawn(move || {
        helper.send(&ControlMessage::Dirty).unwrap();
        let request = helper.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(request, ControlMessage::GuiShow(title) if title == "Pad"));
        let relay = ParamRelay {
            events: vec![ParamEvent {
                id: 1,
                kind: ParamEventKind::Begin,
            }],
            ..ParamRelay::default()
        };
        // A notice sent before the reply must not be taken as the reply.
        helper.send(&ControlMessage::ParamRelay(relay)).unwrap();
        helper
            .send(&ControlMessage::GuiShown(GuiReport {
                created: true,
                visible: true,
                key: false,
            }))
            .unwrap();
        helper.send(&ControlMessage::Dirty).unwrap();
        helper.send(&ControlMessage::GuiClosed).unwrap();
        helper.send(&ControlMessage::Dirty).unwrap();
        helper
    });
    let report = control.show_gui("Pad").unwrap();
    assert!(report.created && !report.key);
    let helper = answer.join().unwrap();
    let mut state = control.lock();
    state.drain().unwrap();
    let dirty = state
        .inbox
        .iter()
        .filter(|message| matches!(message, ControlMessage::Dirty))
        .count();
    assert_eq!(dirty, 3, "every dirty notice stays for the background save");
    drop(state);
    let activity = control.take_activity().unwrap();
    assert_eq!(activity.relay.events.len(), 1);
    assert!(activity.dirty, "the dirty notice while the window was open");
    assert!(
        !activity.window_open,
        "the relay arrived before the window opened"
    );
    assert!(activity.closed);
    assert!(control.take_activity().is_none());
    drop(helper);
}

#[test]
fn contract_window_lifetimes_cross_the_control_socket_in_order() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    let shown = ControlMessage::GuiShown(GuiReport {
        created: true,
        visible: false,
        key: false,
    });
    // A successfully created but hidden window is open; visibility is not its lifetime.
    helper.send(&shown).unwrap();
    control.lock().drain().unwrap();
    let opened = control.take_activity().unwrap();
    assert_eq!(opened.window_state, Some(true));
    assert!(
        !opened.window_open,
        "no edit arrived while the window was open"
    );
    assert!(control.take_activity().is_none());

    // A close followed by a reopen in the same unread page must show as open,
    // even though the history still needs the earlier close and relay flags.
    helper.send(&ControlMessage::GuiClosed).unwrap();
    helper.send(&shown).unwrap();
    helper.send(&ControlMessage::Dirty).unwrap();
    control.lock().drain().unwrap();
    let reopened = control.take_activity().unwrap();
    assert!(reopened.closed && reopened.dirty);
    assert_eq!(reopened.window_state, Some(true));
    helper.send(&ControlMessage::GuiClosedReply).unwrap();
    control.lock().drain().unwrap();
    assert_eq!(control.take_activity().unwrap().window_state, Some(false));

    // A rejected request does not light the indicator. Socket death closes it
    // even when complete replies remain in the inbox for another requester.
    helper
        .send(&ControlMessage::GuiRejected("no window".into()))
        .unwrap();
    control.lock().drain().unwrap();
    assert!(control.take_activity().is_none());
    helper.send(&shown).unwrap();
    control.lock().drain().unwrap();
    assert_eq!(control.take_activity().unwrap().window_state, Some(true));
    drop(helper);
    let _ = control.lock().drain();
    assert_eq!(control.take_activity().unwrap().window_state, Some(false));
    let _ = control.lock().drain();
    assert!(control.take_activity().is_none());
}

#[test]
fn contract_plugin_shortcuts_cross_the_control_socket_once() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    helper
        .send(&ControlMessage::GuiShown(GuiReport {
            created: true,
            visible: true,
            key: false,
        }))
        .unwrap();
    for command in [
        crate::PluginCommand::Playback,
        crate::PluginCommand::Pause,
        crate::PluginCommand::Undo,
        crate::PluginCommand::Redo,
    ] {
        helper.send(&ControlMessage::GuiCommand(command)).unwrap();
    }
    control.lock().drain().unwrap();
    let activity = control.take_activity().unwrap();
    assert!(activity.window_open);
    assert_eq!(
        activity.commands,
        [
            crate::PluginCommand::Playback,
            crate::PluginCommand::Pause,
            crate::PluginCommand::Undo,
            crate::PluginCommand::Redo
        ]
    );
    assert!(control.take_activity().is_none());
    assert!(ControlMessage::GuiCommand(crate::PluginCommand::Playback).is_notice());
}

#[test]
fn spawn_sources_stay_above_the_child_fds() {
    let fan = FanIn::create().unwrap();
    let dup = inheritable(fan.raw_fd()).unwrap();
    assert!(dup.as_raw_fd() >= 6);
}

#[test]
fn stderr_keeps_the_tail() {
    let mut slot = String::new();
    push_stderr(&mut slot, &"a".repeat(70_000));
    assert!(slot.len() >= 64 * 1024);
    assert!(slot.len() <= 64 * 1024 + 3);
    assert!(slot.chars().all(|ch| ch == 'a'));
}
