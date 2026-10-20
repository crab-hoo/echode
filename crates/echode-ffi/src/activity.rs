//! Plugin activity for the app: window notices and the changes each plugin
//! made on its own (ADR 0132). The document worker publishes what it collected
//! from the helpers; the app reads it on its own thread and never waits for
//! the worker here.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use echode_audio_io::{ParamEventKind, PluginActivity};
use serde::Serialize;

/// Entries kept for a reader that stopped reading. Beyond this, the backlog
/// collapses to one loss per device: the reader then reads each list again
/// and records the plugin's state, so no change is silently dropped.
const BACKLOG: usize = 4096;

pub(crate) struct ActivityHub {
    state: Mutex<HubState>,
    ready: Condvar,
}

#[derive(Default)]
struct HubState {
    /// Sequence number of the newest entry.
    newest: u64,
    entries: VecDeque<Entry>,
    closed: bool,
}

struct Entry {
    seq: u64,
    device: u128,
    activity: PluginActivity,
}

#[derive(Serialize)]
struct Page<'a> {
    next: u64,
    closed: bool,
    entries: Vec<EntryJson<'a>>,
}

#[derive(Serialize)]
struct EntryJson<'a> {
    device: String,
    events: Vec<EventJson>,
    values: Vec<ValueJson<'a>>,
    rescan: bool,
    lost: bool,
    dirty: bool,
    closed: bool,
    window: bool,
}

#[derive(Serialize)]
struct EventJson {
    id: u32,
    kind: &'static str,
}

#[derive(Serialize)]
struct ValueJson<'a> {
    id: u32,
    value: f64,
    text: &'a str,
}

impl ActivityHub {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(HubState::default()),
            ready: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn publish(&self, device: u128, activity: PluginActivity) {
        let mut state = self.lock();
        if state.entries.len() >= BACKLOG {
            collapse(&mut state);
        }
        state.newest += 1;
        let seq = state.newest;
        state.entries.push_back(Entry {
            seq,
            device,
            activity,
        });
        drop(state);
        self.ready.notify_all();
    }

    /// The document closed; a waiting reader returns at once.
    pub(crate) fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// Drops entries up to `after`, which the reader already has, waits up to
    /// `timeout` for a newer one, and returns the page as JSON.
    pub(crate) fn read(&self, after: u64, timeout: Duration) -> Vec<u8> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        while state
            .entries
            .front()
            .is_some_and(|entry| entry.seq <= after)
        {
            state.entries.pop_front();
        }
        while state.entries.is_empty() && !state.closed {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self
                .ready
                .wait_timeout(state, left)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
        let page = Page {
            next: state.entries.back().map_or(after, |entry| entry.seq),
            closed: state.closed,
            entries: state.entries.iter().map(entry_json).collect(),
        };
        serde_json::to_vec(&page).unwrap_or_else(|_| b"{}".to_vec())
    }
}

/// Keeps one entry per device that says events were lost.
fn collapse(state: &mut HubState) {
    let mut kept: Vec<Entry> = Vec::new();
    for entry in state.entries.drain(..) {
        let closed = entry.activity.closed;
        match kept.iter_mut().find(|known| known.device == entry.device) {
            Some(known) => {
                known.seq = entry.seq;
                known.activity.closed |= closed;
            }
            None => {
                let mut activity = PluginActivity {
                    closed,
                    ..PluginActivity::default()
                };
                activity.relay.lost = true;
                activity.relay.rescan = true;
                kept.push(Entry {
                    seq: entry.seq,
                    device: entry.device,
                    activity,
                });
            }
        }
    }
    kept.sort_by_key(|entry| entry.seq);
    state.entries = kept.into();
}

fn entry_json(entry: &Entry) -> EntryJson<'_> {
    let relay = &entry.activity.relay;
    EntryJson {
        device: uuid::Uuid::from_u128(entry.device).to_string(),
        events: relay
            .events
            .iter()
            .map(|event| EventJson {
                id: event.id,
                kind: match event.kind {
                    ParamEventKind::Value => "value",
                    ParamEventKind::Begin => "begin",
                    ParamEventKind::End => "end",
                },
            })
            .collect(),
        values: relay
            .values
            .iter()
            .map(|value| ValueJson {
                id: value.id,
                value: value.value,
                text: &value.text,
            })
            .collect(),
        rescan: relay.rescan,
        lost: relay.lost,
        dirty: entry.activity.dirty,
        closed: entry.activity.closed,
        window: entry.activity.window_open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
