use super::*;

#[test]
fn latency_survives_activity_json_and_backlog_collapse() {
    let hub = ActivityHub::new();
    // The count exceeds only this reader backlog, not a device limit.
    for index in 0..BACKLOG + 1 {
        hub.publish(
            0xD0,
            PluginActivity {
                latency_samples: match index {
                    0 => Some(512),
                    1 => Some(0),
                    _ => None,
                },
                ..PluginActivity::default()
            },
        );
    }
    let received = page(&hub.read(0, Duration::ZERO));
    assert_eq!(received["entries"][0]["latency_samples"], 0);
    assert!(received["entries"][1]["latency_samples"].is_null());
}
use echode_audio_io::{ParamEvent, ParamValue};

fn gesture_end(value: f64) -> PluginActivity {
    let mut activity = PluginActivity::default();
    activity.relay.events.push(ParamEvent {
        id: 1,
        kind: ParamEventKind::End,
    });
    activity.relay.values.push(ParamValue {
        id: 1,
        value,
        text: format!("{value:.2}x"),
    });
    activity
}

fn page(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn gui_commands_reach_the_activity_json_in_order() {
    let hub = ActivityHub::new();
    hub.publish(
        0xD0,
        PluginActivity {
            commands: vec![
                echode_audio_io::PluginCommand::Playback,
                echode_audio_io::PluginCommand::Undo,
            ],
            window_open: true,
            ..PluginActivity::default()
        },
    );
    let received = page(&hub.read(0, Duration::ZERO));
    assert_eq!(
        received["entries"][0]["commands"],
        serde_json::json!(["Play / Stop", "Undo"])
    );
    assert_eq!(received["entries"][0]["window"], true);
    assert!(
        page(&hub.read(1, Duration::ZERO))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn window_lifetimes_survive_json_and_backlog_collapse_in_order() {
    let hub = ActivityHub::new();
    for (index, state) in [Some(true), Some(false), Some(true), None]
        .into_iter()
        .enumerate()
    {
        hub.publish(
            0xD0,
            PluginActivity {
                window_state: state,
                ..PluginActivity::default()
            },
        );
        let received = page(&hub.read(index as u64, Duration::ZERO));
        assert_eq!(
            received["entries"][0]["window_state"],
            serde_json::json!(state)
        );
    }
    // Counts describe only this backlog regression, not a device/track limit.
    for index in 0..BACKLOG + 1 {
        let state = match index {
            0 | 2 => Some(true),
            1 => Some(false),
            _ => None,
        };
        hub.publish(
            0xD0,
            PluginActivity {
                window_state: state,
                ..PluginActivity::default()
            },
        );
    }
    let collapsed = page(&hub.read(4, Duration::ZERO));
    assert_eq!(collapsed["entries"][0]["window_state"], true);
    assert_eq!(collapsed["entries"][0]["lost"], true);
    assert!(collapsed["entries"][1]["window_state"].is_null());
}

#[test]
fn a_reader_gets_each_entry_until_it_moves_past_it() {
    let hub = ActivityHub::new();
    let quiet = page(&hub.read(0, Duration::from_millis(1)));
    assert_eq!(quiet["next"], 0);
    assert_eq!(quiet["entries"].as_array().unwrap().len(), 0);
    hub.publish(0xD0, gesture_end(1.5));
    let first = page(&hub.read(0, Duration::from_secs(1)));
    assert_eq!(first["next"], 1);
    let entry = &first["entries"][0];
    assert_eq!(entry["device"], "00000000-0000-0000-0000-0000000000d0");
    assert_eq!(entry["events"][0]["kind"], "end");
    assert_eq!(entry["values"][0]["text"], "1.50x");
    // Asking again from the same point returns the same entry.
    assert_eq!(page(&hub.read(0, Duration::ZERO))["next"], 1);
    assert_eq!(
        page(&hub.read(1, Duration::from_millis(1)))["entries"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    hub.close();
    assert_eq!(page(&hub.read(1, Duration::from_secs(5)))["closed"], true);
}

#[test]
fn a_waiting_reader_wakes_for_a_new_entry() {
    let hub = std::sync::Arc::new(ActivityHub::new());
    let reader = {
        let hub = std::sync::Arc::clone(&hub);
        std::thread::spawn(move || page(&hub.read(0, Duration::from_secs(10))))
    };
    std::thread::sleep(Duration::from_millis(20));
    hub.publish(0xD0, gesture_end(0.5));
    assert_eq!(reader.join().unwrap()["next"], 1);
}

#[test]
fn an_unread_backlog_collapses_to_a_loss_per_device() {
    let hub = ActivityHub::new();
    for index in 0..BACKLOG + 1 {
        hub.publish(0xD0 + (index % 2) as u128, gesture_end(1.0));
    }
    let collapsed = page(&hub.read(0, Duration::ZERO));
    let entries = collapsed["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        3,
        "one loss for each device, then the newest"
    );
    assert!(entries[..2].iter().all(|entry| entry["lost"] == true));
    assert_eq!(collapsed["next"], (BACKLOG + 1) as u64);
}

/// DOC-A1 path 2: a change made in an open window is still a window edit
/// after the backlog collapses, so the reader records it once it is quiet.
#[test]
fn a_collapsed_backlog_keeps_an_open_window_and_a_dirty_state() {
    let hub = ActivityHub::new();
    hub.publish(
        0xD0,
        PluginActivity {
            dirty: true,
            window_open: true,
            ..PluginActivity::default()
        },
    );
    hub.publish(0xD1, gesture_end(0.5));
    for _ in 0..BACKLOG {
        hub.publish(0xD1, gesture_end(1.0));
    }
    let collapsed = page(&hub.read(0, Duration::ZERO));
    let entries = collapsed["entries"].as_array().unwrap();
    let window = entries
        .iter()
        .find(|entry| entry["device"] == uuid::Uuid::from_u128(0xD0).to_string())
        .unwrap();
    assert_eq!(window["lost"], true);
    assert_eq!(window["window"], true, "the window edit was forgotten");
    assert_eq!(window["dirty"], true);
    let closed = entries
        .iter()
        .find(|entry| entry["device"] == uuid::Uuid::from_u128(0xD1).to_string())
        .unwrap();
    assert_eq!(closed["window"], false);
    assert_eq!(closed["dirty"], false);
}
