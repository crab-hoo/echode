//! Whole-file overviews for the sample browser (ADR 0163).
//!
//! Each call reads on the caller's thread. Within one channel (one browser),
//! a call with a larger request number stops older ones between reads, so
//! moving through a list does not queue up full decodes of files that are no
//! longer selected. Other channels are not affected. Ending a channel stops
//! its reads.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// Newest request per open channel. One entry per browser, not per file.
static LATEST: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());

fn latest() -> std::sync::MutexGuard<'static, BTreeMap<u64, u64>> {
    LATEST.lock().unwrap_or_else(|error| error.into_inner())
}

fn begin(channel: u64, request: u64) {
    let mut channels = latest();
    let newest = channels.entry(channel).or_insert(request);
    *newest = (*newest).max(request);
}

fn live(channel: u64, request: u64) -> bool {
    latest()
        .get(&channel)
        .is_some_and(|newest| *newest <= request)
}

pub(crate) fn end(channel: u64) {
    latest().remove(&channel);
}

pub(crate) fn overview_json(path: &Path, channel: u64, request: u64) -> Vec<u8> {
    begin(channel, request);
    let value = match echode_audio_io::sample_overview(path, &|| live(channel, request)) {
        Ok(Some(waveform)) => serde_json::json!({ "waveform": waveform }),
        Ok(None) => serde_json::json!({ "cancelled": true }),
        Err(error) => serde_json::json!({ "error": error.to_string() }),
    };
    serde_json::to_vec(&value)
        .unwrap_or_else(|_| br#"{"error":"overview did not encode"}"#.to_vec())
}

#[cfg(test)]
mod tests {
    use super::{begin, end, live};

    #[test]
    fn a_newer_request_stops_only_its_own_channel() {
        // Channel numbers far from the app's, so parallel tests stay apart.
        let (first, second) = (u64::MAX - 1, u64::MAX - 2);
        begin(first, 1);
        begin(second, 1);
        assert!(live(first, 1) && live(second, 1));
        begin(first, 2);
        assert!(!live(first, 1) && live(first, 2));
        assert!(live(second, 1), "another browser's read was stopped");
        end(first);
        assert!(!live(first, 2), "an ended channel keeps reading");
        assert!(live(second, 1));
        end(second);
    }
}
