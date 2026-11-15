use super::{FanIn, PluginControl, inheritable, push_stderr};
use crate::control::{ControlMessage, Socket};
use crate::gui::{GuiReport, ParamEvent, ParamEventKind, ParamRelay};
use crate::refusal::Refusal;
use std::os::fd::AsRawFd;
use std::time::Duration;

/// Window notices and plugin-made changes leave the inbox, so a request
/// waiting for its reply never takes them, and a dirty notice counts as
/// the user's change only while the window is open. That change says the
/// window was open, so the app records it once the plugin is quiet (DOC-A1).
#[test]
fn window_activity_is_kept_apart_from_replies() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    let (go, opened) = std::sync::mpsc::channel::<()>();
    let answer = std::thread::spawn(move || {
        helper.send(&ControlMessage::Dirty).unwrap();
        let (id, request) = helper.recv_tagged(Duration::from_secs(5)).unwrap();
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
            .send_tagged(
                id,
                &ControlMessage::GuiShown(GuiReport {
                    created: true,
                    visible: true,
                    key: false,
                }),
            )
            .unwrap();
        opened.recv().unwrap();
        helper.send(&ControlMessage::Dirty).unwrap();
        helper.send(&ControlMessage::GuiClosed).unwrap();
        helper.send(&ControlMessage::Dirty).unwrap();
        helper
    });
    let report = control.show_gui("Pad").unwrap();
    assert!(report.created && !report.key);
    let before = control.take_activity().unwrap();
    assert_eq!(before.relay.events.len(), 1);
    assert!(
        !before.dirty,
        "the dirty notice came before the window opened"
    );
    assert!(
        !before.window_open,
        "the relay arrived before the window opened"
    );
    go.send(()).unwrap();
    let helper = answer.join().unwrap();
    let mut state = control.lock();
    state.drain().unwrap();
    assert!(
        state.dirty_notice,
        "coalesced dirty notice stays for the background save"
    );
    assert!(
        state.inbox.is_empty(),
        "dirty notices do not stack in the reply inbox"
    );
    drop(state);
    let activity = control.take_activity().unwrap();
    assert!(activity.relay.is_empty());
    assert!(activity.dirty, "the dirty notice while the window was open");
    assert!(activity.window_open, "a dirty notice is a window edit");
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
        .send(&ControlMessage::GuiRejected(Refusal::Plugin(
            crate::PluginFailure::NoWindow,
        )))
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

/// DIA-04: a busy capture has not saved the change. Keep it pending, wait
/// between attempts, and collect a later reply without another dirty notice.
#[test]
fn audit_dia04_a_busy_refusal_keeps_the_plugins_change_waiting_for_a_save() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let mut control = PluginControl::new(parent);
    let now = std::time::Instant::now();
    let wait = Duration::from_secs(5);
    helper.send(&ControlMessage::Dirty).unwrap();
    assert!(control.poll_state_save_at(now).is_none());
    let (id, request) = helper.recv_tagged(wait).unwrap();
    assert!(matches!(request, ControlMessage::SaveRequest));
    helper
        .send_tagged(id, &ControlMessage::StateRejected(Refusal::Busy))
        .unwrap();
    assert!(control.poll_state_save_at(now).is_none());
    for turn in 0..50 {
        assert!(
            control
                .poll_state_save_at(now + Duration::from_millis(turn * 20))
                .is_none()
        );
        assert!(
            helper.try_recv_tagged().unwrap().is_none(),
            "capture retried too soon"
        );
    }
    assert!(
        control
            .poll_state_save_at(now + super::SAVE_RETRY)
            .is_none()
    );
    let Some((retry, request)) = helper.try_recv_tagged().unwrap() else {
        panic!("the change is still only in the plugin and no save was asked for again");
    };
    assert!(retry > id);
    assert!(matches!(request, ControlMessage::SaveRequest));
    helper
        .send_tagged(retry, &ControlMessage::State(vec![42]))
        .unwrap();
    assert_eq!(
        control.poll_state_save_at(now + super::SAVE_RETRY),
        Some(vec![42])
    );
    assert_eq!(control.applied_state().unwrap().bytes(), [42]);
    assert!(
        control
            .poll_state_save_at(now + super::SAVE_RETRY)
            .is_none()
    );
    assert!(helper.try_recv_tagged().unwrap().is_none());
}

/// DIA-10: unused process wakeups drop on arrival, pending restart notices
/// stay intact without stalling dirty captures behind them, and duplicate
/// dirty notices coalesce to one background save on the first poll.
#[test]
fn audit_dia10_dirty_notices_coalesce_ahead_of_unused_and_restart_notices() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let mut control = PluginControl::new(parent);
    let now = std::time::Instant::now();
    for _ in 0..4 {
        helper.send(&ControlMessage::RequestProcess).unwrap();
    }
    for latency in [32, 64] {
        helper
            .send(&ControlMessage::PluginRestarted {
                latency_samples: latency,
            })
            .unwrap();
    }
    for _ in 0..5 {
        helper.send(&ControlMessage::Dirty).unwrap();
    }

    // One poll coalesces every queued Dirty immediately, even behind restarts.
    assert!(control.poll_state_save_at(now).is_none());
    let Some((id, ControlMessage::SaveRequest)) = helper.try_recv_tagged().unwrap() else {
        panic!("expected immediate SaveRequest coalesced ahead of restart notices");
    };
    assert!(helper.try_recv_tagged().unwrap().is_none());

    // Restart notices remain available in order and RequestProcess was dropped.
    {
        let mut state = control.lock();
        assert!(state.inbox.is_empty());
        assert_eq!(
            state.poll_notice().unwrap(),
            Some(super::PluginNotice::PluginRestarted {
                latency_samples: 32
            })
        );
        assert_eq!(
            state.poll_notice().unwrap(),
            Some(super::PluginNotice::PluginRestarted {
                latency_samples: 64
            })
        );
        assert_eq!(state.poll_notice().unwrap(), None);
    }

    helper
        .send_tagged(id, &ControlMessage::State(vec![7]))
        .unwrap();
    assert_eq!(control.poll_state_save_at(now), Some(vec![7]));
    // No duplicate dirty notices remain queued to trigger a second save.
    assert!(control.poll_state_save_at(now).is_none());
    assert!(helper.try_recv_tagged().unwrap().is_none());
}

/// DIA-10: inbox, restart notices, and each `drain()` call stay bounded even
/// when a helper sends unsolicited notices without pause.
#[test]
fn audit_dia10_control_drain_and_queues_stay_bounded_under_continuous_notices() {
    let (parent, helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    {
        let mut state = control.lock();
        for index in 0..200 {
            state.accept(
                0,
                ControlMessage::PluginRestarted {
                    latency_samples: index,
                },
            );
            state.accept(
                0,
                ControlMessage::Ports {
                    inputs: vec![2],
                    outputs: vec![2],
                    tail_samples: 0,
                    latency_samples: index,
                },
            );
        }
        assert_eq!(state.restart_notices.len(), super::MAX_RESTART_NOTICES);
        assert_eq!(state.inbox.len(), super::MAX_INBOX);
    }

    let (parent, mut helper_flood) = Socket::pair().unwrap();
    drop(helper);
    let mut flooding_control = PluginControl::new(parent);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let writer = {
        let stop = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut sent = 0usize;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if helper_flood.send(&ControlMessage::Dirty).is_err() {
                    break;
                }
                sent += 1;
                if sent == super::DRAIN_LIMIT * 2 {
                    let _ = ready_tx.send(());
                }
            }
        })
    };
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    // Even under continuous traffic, one poll finishes cleanly and wait_reply
    // reaches its deadline instead of spinning indefinitely inside drain().
    assert!(
        flooding_control
            .poll_state_save_at(std::time::Instant::now())
            .is_none()
    );
    let mut state = flooding_control.lock();
    let reply_id = state
        .request(
            ControlMessage::SaveRequest,
            std::time::Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_millis(20);
    let err = state.wait_reply(reply_id, deadline, |_| true).unwrap_err();
    assert!(matches!(err, super::SessionError::Timeout), "{err:?}");

    stop.store(true, std::sync::atomic::Ordering::Release);
    drop(state);
    drop(flooding_control);
    let _ = writer.join();
}
