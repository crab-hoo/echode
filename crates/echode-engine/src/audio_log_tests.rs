use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use echode_model::{Id, Project};
use serde_json::{Value, json};

use super::*;
use crate::meter::{AutoMute, TrackMeter};

fn messages(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|record| record["message"].as_str())
        .collect()
}

struct Fixture {
    project: Project,
    track: Id,
    device: Id,
    meters: BTreeMap<u128, Arc<TrackMeter>>,
}

fn fixture() -> Fixture {
    let (project, track) = Project::new().add_track("logged");
    let device = Id::generate();
    let project = project
        .add_clap_device(track, device, "dev.echode.test-gain")
        .unwrap();
    Fixture {
        project,
        track,
        device,
        meters: BTreeMap::new(),
    }
}

impl Fixture {
    fn scene<'a>(&'a self, exited: &'a dyn Fn(u128) -> bool) -> Scene<'a> {
        Scene {
            project: &self.project,
            meters: &self.meters,
            master: None,
            exited,
        }
    }
}

#[test]
fn a_missing_plugin_is_named_at_once_then_counted_until_it_goes_quiet() {
    let capture = echode_log::testing::capture();
    let fixture = fixture();
    let never = |_| false;
    let scene = fixture.scene(&never);
    let (mut notes, mut log) = note_pair();
    let device = fixture.device.raw();
    let miss = |sample| Note::Plugin {
        device,
        reason: MISS_DEADLINE,
        sample,
    };
    let start = Instant::now();
    for sample in [100, 228, 356] {
        notes.send(miss(sample));
    }
    log.drain(start, &scene);
    let records = capture.on_this_thread();
    let [first] = records.as_slice() else {
        panic!("one line names the node at once: {records:?}");
    };
    assert_eq!(first["level"], "warn");
    assert_eq!(first["target"], "echode_engine::audio");
    assert_eq!(first["message"], "a plugin did not finish its audio block");
    assert_eq!(
        first["fields"],
        json!({
            "device": fixture.device.to_string(),
            "track": fixture.track.to_string(),
            "plugin": "dev.echode.test-gain",
            "reason": "deadline",
            "sample": 100,
        })
    );

    // Further misses within the episode are only counted.
    notes.send(miss(484));
    notes.send(miss(612));
    log.drain(start + Duration::from_secs(5), &scene);
    assert_eq!(capture.on_this_thread().len(), 1);

    log.drain(start + Duration::from_secs(11), &scene);
    let records = capture.on_this_thread();
    assert_eq!(
        messages(&records),
        [
            "a plugin did not finish its audio block",
            "a plugin kept missing audio blocks"
        ]
    );
    let summary = &records[1]["fields"];
    assert_eq!(summary["count"], 4);
    assert_eq!(summary["first_sample"], 228);
    assert_eq!(summary["last_sample"], 612);
    assert_eq!(summary["plugin"], "dev.echode.test-gain");

    // Quiet for the summary period ends the episode; the next miss is named again.
    log.drain(start + Duration::from_secs(16), &scene);
    notes.send(miss(740));
    log.drain(start + Duration::from_secs(17), &scene);
    let records = capture.on_this_thread();
    assert_eq!(records.len(), 3);
    assert_eq!(
        records[2]["message"],
        "a plugin did not finish its audio block"
    );
    assert_eq!(records[2]["fields"]["sample"], 740);
}

#[test]
fn a_deadline_miss_of_an_exited_helper_is_logged_as_its_exit_and_close_reports_pending_counts() {
    let capture = echode_log::testing::capture();
    let fixture = fixture();
    let device = fixture.device.raw();
    let gone = move |candidate| candidate == device;
    let scene = fixture.scene(&gone);
    let (mut notes, mut log) = note_pair();
    for sample in [0, 64, 128] {
        notes.send(Note::Plugin {
            device,
            reason: MISS_DEADLINE,
            sample,
        });
    }
    notes.send(Note::Late {
        sample: 64,
        late_ns: 250_000,
        frames: 64,
    });
    notes.send(Note::Late {
        sample: 128,
        late_ns: 900_000,
        frames: 64,
    });
    log.drain(Instant::now(), &scene);
    log.finish(&scene);
    let records = capture.on_this_thread();
    assert_eq!(
        messages(&records),
        [
            "a plugin did not finish its audio block",
            "an audio block finished after its deadline",
            "a plugin kept missing audio blocks",
            "more audio blocks finished after their deadline",
        ]
    );
    assert_eq!(records[0]["fields"]["reason"], "helper exited");
    assert_eq!(records[1]["fields"]["late_us"], 250);
    assert_eq!(records[2]["fields"]["count"], 2);
    assert_eq!(records[3]["fields"]["count"], 1);
    assert_eq!(records[3]["fields"]["worst_late_us"], 900);
}

#[test]
fn a_full_ring_drops_notes_without_blocking_and_the_log_counts_them() {
    let capture = echode_log::testing::capture();
    let fixture = fixture();
    let never = |_| false;
    let scene = fixture.scene(&never);
    let (mut notes, mut log) = note_pair();
    let guard = crate::ForbidAlloc::enter();
    for sample in 0..(NOTES as i64 + 5) {
        notes.send(Note::Late {
            sample,
            late_ns: 1_000,
            frames: 32,
        });
    }
    drop(guard);
    log.drain(Instant::now(), &scene);
    let records = capture.on_this_thread();
    assert_eq!(
        messages(&records),
        [
            "an audio block finished after its deadline",
            "the audio thread's log ring was full; later drops are counted"
        ]
    );
    assert_eq!(records[1]["fields"]["dropped"], 5);
}

#[test]
fn automatic_mutes_are_logged_when_they_start_and_end() {
    let capture = echode_log::testing::capture();
    let mut fixture = fixture();
    let meter = Arc::new(TrackMeter::new(fixture.track.raw()));
    fixture
        .meters
        .insert(fixture.track.raw(), Arc::clone(&meter));
    let never = |_| false;
    let (_notes, mut log) = note_pair();
    log.drain(Instant::now(), &fixture.scene(&never));
    assert!(capture.on_this_thread().is_empty());

    meter.set_auto_mute(AutoMute {
        device: Some(fixture.device.raw()),
        at: 4_800,
    });
    let mut scene = fixture.scene(&never);
    scene.master = Some((4.0, 9_600));
    log.drain(Instant::now(), &scene);
    log.drain(Instant::now(), &scene);
    meter.clear_auto_mute();
    log.drain(Instant::now(), &fixture.scene(&never));
    let records = capture.on_this_thread();
    assert_eq!(
        messages(&records),
        [
            "a track was muted because a plugin returned a non-finite sample",
            "the master output was muted after a peak above the guard",
            "a track's automatic mute ended",
            "the master output's automatic mute ended",
        ]
    );
    assert_eq!(records[0]["fields"]["device"], fixture.device.to_string());
    assert_eq!(records[0]["fields"]["plugin"], "dev.echode.test-gain");
    assert_eq!(records[0]["fields"]["sample"], 4_800);
    assert_eq!(records[1]["fields"]["peak"], 4.0);
}

#[test]
fn a_host_mix_mute_logs_no_device_or_plugin_and_only_once() {
    let capture = echode_log::testing::capture();
    let mut fixture = fixture();
    let meter = Arc::new(TrackMeter::new(fixture.track.raw()));
    fixture
        .meters
        .insert(fixture.track.raw(), Arc::clone(&meter));
    let never = |_| false;
    let (_notes, mut log) = note_pair();
    meter.set_auto_mute(AutoMute {
        device: None,
        at: 11,
    });
    for _ in 0..2 {
        log.drain(Instant::now(), &fixture.scene(&never));
    }
    meter.clear_auto_mute();
    log.drain(Instant::now(), &fixture.scene(&never));
    let records = capture.on_this_thread();
    assert_eq!(
        messages(&records),
        [
            "a track was muted because its audio sum produced a non-finite sample",
            "a track's automatic mute ended",
        ]
    );
    let fields = &records[0]["fields"];
    assert_eq!(fields["track"], fixture.track.to_string());
    assert_eq!(fields["sample"], 11);
    assert!(fields.get("device").is_none() && fields.get("plugin").is_none());
}
