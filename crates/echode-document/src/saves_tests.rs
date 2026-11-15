use super::*;
use echode_audio_io::{ParamEvent, ParamEventKind};
use echode_model::Id;
use std::time::Duration;

const FX: u128 = 0xF1;
const SYNTH: u128 = 0xF2;

fn refused() -> HostError {
    HostError::Package(echode_history::PackageError::from(std::io::Error::other(
        "disk full",
    )))
}

fn with_devices() -> Project {
    let (project, track) = Project::new().add_track("fx");
    project
        .add_clap_device(track, Id::from_raw(FX), "org.example.fx")
        .unwrap()
        .add_clap_device(track, Id::from_raw(SYNTH), "org.example.fx")
        .unwrap()
}

#[test]
fn a_save_waits_for_the_edit_of_its_own_device_only() {
    let mut saves = PluginSaves::default();
    let mut gesture = PluginActivity::default();
    gesture.relay.events.push(ParamEvent {
        id: 1,
        kind: ParamEventKind::Begin,
    });
    saves.note_activity(FX, &gesture);
    saves.add(vec![(FX, b"edited".to_vec()), (SYNTH, b"other".to_vec())]);
    let mut seen = Vec::new();
    assert!(saves.record(|device, _| {
        seen.push(device);
        Ok(true)
    }));
    assert_eq!(seen, [SYNTH]);
    saves.editing.remove(&FX);
    assert!(saves.record(|device, _| {
        seen.push(device);
        Ok(true)
    }));
    assert_eq!(seen, [SYNTH, FX]);
    assert!(saves.pending.is_empty());
}

#[test]
fn a_slow_failed_commit_waits_from_completion_and_does_not_block_other_devices() {
    let mut saves = PluginSaves::default();
    saves.add(vec![(FX, vec![1]), (SYNTH, vec![2])]);
    let start = Instant::now();
    let clock = std::cell::Cell::new(start);
    let mut seen = Vec::new();
    assert!(saves.record_with_clock(
        || clock.get(),
        |device, _| {
            seen.push(device);
            if device == FX {
                clock.set(start + Duration::from_secs(10));
                Err(refused())
            } else {
                Ok(true)
            }
        }
    ));
    assert_eq!(seen, [FX, SYNTH]);
    assert!(
        !saves.record_at(start + Duration::from_secs(10), |_, _| panic!(
            "retried before the completion delay"
        ))
    );
    assert!(saves.record_at(start + Duration::from_millis(10_100), |_, _| Ok(true)));
}

#[test]
fn unknown_durability_keeps_every_capture_and_stops_this_turn() {
    let mut saves = PluginSaves::default();
    saves.add(vec![(FX, vec![1]), (SYNTH, vec![2])]);
    let mut attempts = 0;
    assert!(!saves.record(|device, _| {
        attempts += 1;
        assert_eq!(device, FX);
        Err(HostError::Package(
            echode_history::PackageError::JournalDurabilityUnknown,
        ))
    }));
    assert_eq!(attempts, 1);
    assert_eq!(saves.held(), [FX, SYNTH]);
}

#[test]
fn failures_are_spaced_and_new_bytes_and_unrelated_edits_keep_the_delay() {
    let before = with_devices();
    let (after, _) = before.add_track("unrelated");
    let mut saves = PluginSaves::default();
    saves.add(vec![(FX, b"first".to_vec())]);
    let now = Instant::now();
    let mut first = None;
    assert!(!saves.record_at(now, |_, state| {
        first = Some(state);
        Err(refused())
    }));
    saves.add(vec![(FX, b"latest".to_vec())]);
    saves.document_changed(&before, &after);
    for _ in 0..100 {
        assert!(!saves.record_at(now, |_, _| panic!("a refused capture bypassed its delay")));
    }
    assert_eq!(saves.pending.len(), 1);
    assert_eq!(
        saves.held(),
        [FX],
        "a refused capture must keep its live helper"
    );
    let latest = Arc::clone(&saves.pending[0].state);
    assert!(!Arc::ptr_eq(first.as_ref().unwrap(), &latest));
    assert!(
        !saves.record_at(now + Duration::from_millis(100), |_, state| {
            assert!(Arc::ptr_eq(&state, &latest));
            Err(refused())
        })
    );
    assert!(
        !saves.record_at(now + Duration::from_millis(299), |_, _| panic!(
            "second delay was not doubled"
        ))
    );
    assert!(
        saves.record_at(now + Duration::from_millis(300), |_, state| {
            assert!(Arc::ptr_eq(&state, &latest));
            Ok(true)
        })
    );
    assert!(saves.pending.is_empty());
}

#[test]
fn only_a_changed_device_drops_its_waiting_capture_and_edit() {
    let before = with_devices();
    let device = Id::from_raw(FX);
    let changed = before
        .set_device_state(device, Some("a".repeat(64)))
        .unwrap();
    let removed = before.remove_device(device).unwrap();
    let offline = before.set_device_offline(device, true).unwrap();
    for after in [changed, removed, offline] {
        let mut saves = PluginSaves::default();
        saves.editing.insert(FX);
        saves.add(vec![(FX, b"old".to_vec()), (SYNTH, b"kept".to_vec())]);
        saves.document_changed(&before, &after);
        assert!(saves.editing.is_empty());
        assert_eq!(saves.held(), [SYNTH]);
        assert_eq!(
            saves
                .pending
                .iter()
                .map(|save| save.device)
                .collect::<Vec<_>>(),
            [SYNTH]
        );
    }
}

#[test]
fn an_unrelated_edit_keeps_the_newest_held_capture() {
    let before = with_devices();
    let (after, _) = before.add_track("unrelated");
    let mut saves = PluginSaves::default();
    saves.editing.insert(FX);
    saves.add(vec![(FX, b"first".to_vec()), (FX, b"latest".to_vec())]);
    let latest = Arc::clone(&saves.pending[0].state);
    saves.document_changed(&before, &after);
    assert_eq!(saves.held(), [FX]);
    saves.editing.remove(&FX);
    assert!(saves.record(|_, state| {
        assert!(Arc::ptr_eq(&state, &latest));
        Ok(true)
    }));
}

#[test]
fn repeated_failure_logs_only_the_first_and_changed_cause() {
    let capture = echode_log::testing::capture();
    let device = Id::generate().raw();
    let mut saves = PluginSaves::default();
    saves.add(vec![(device, vec![1])]);
    let now = Instant::now();
    for millis in [0, 100, 300] {
        saves.record_at(now + Duration::from_millis(millis), |_, _| Err(refused()));
    }
    saves.record_at(now + Duration::from_millis(700), |_, _| {
        Err(HostError::Package(echode_history::PackageError::from(
            std::io::Error::other("changed cause"),
        )))
    });
    let records = capture
        .on_this_thread()
        .into_iter()
        .filter(|record| record["target"] == "echode_document::saves")
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert!(saves.record_at(now + Duration::from_millis(1500), |_, _| Ok(true)));
    saves.add(vec![(device, vec![2])]);
    saves.record_at(now + Duration::from_millis(1500), |_, _| Err(refused()));
    assert_eq!(
        capture
            .on_this_thread()
            .into_iter()
            .filter(|record| record["target"] == "echode_document::saves")
            .count(),
        3
    );
}
