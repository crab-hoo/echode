use echode_history::spike::{History, Project, SPIKE_TRACKS};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn scratch() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("echode-history-{}-{n}", std::process::id()))
}

#[test]
fn an_edit_roundtrips_through_an_older_revision() {
    let directory = scratch();
    let history = History::create(&directory).expect("create");
    let original = Project::benchmark();
    let first = history
        .save(Arc::clone(&original))
        .expect("schedule")
        .wait()
        .expect("commit");
    let edited = original.with_gain(0, -3500);
    let second = history
        .save(Arc::clone(&edited))
        .expect("schedule")
        .wait()
        .expect("commit");
    assert_ne!(first, second);

    let loaded = history.load(first).expect("load first");
    assert_eq!(loaded.tracks.len(), SPIKE_TRACKS);
    assert_eq!(loaded.tracks[0].gain_milli_db, 0);
    assert_eq!(loaded.tracks[0].notes, original.tracks[0].notes);

    let loaded = history.load(second).expect("load second");
    assert_eq!(loaded.tracks[0].gain_milli_db, -3500);
    assert_eq!(loaded.tracks[1..], edited.tracks[1..]);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
#[ignore = "timing measurement; `make spike-history` runs it in a release build"]
fn scheduling_a_thousand_track_save_stays_off_the_caller_and_reload_is_under_a_second() {
    let directory = scratch();
    let history = History::create(&directory).expect("create");
    let project = Project::benchmark();

    let started = Instant::now();
    let pending = history.save(Arc::clone(&project)).expect("schedule");
    let scheduled = started.elapsed();
    let commit_started = Instant::now();
    let first = pending.wait().expect("initial commit");
    let initial_commit = commit_started.elapsed();

    let edited = project.with_gain(500, 1250);
    let started = Instant::now();
    let pending = history.save(edited).expect("schedule edited");
    let rescheduled = started.elapsed();
    let commit_started = Instant::now();
    let second = pending.wait().expect("edited commit");
    let edit_commit = commit_started.elapsed();

    let started = Instant::now();
    let loaded = history.load(first).expect("reload");
    let loaded_in = started.elapsed();
    assert_eq!(loaded.tracks[500].gain_milli_db, 0);
    assert_ne!(first, second);

    let report = format!(
        "tracks: {SPIKE_TRACKS}\n\
         schedule initial: {sched:.3} ms\n\
         schedule edit: {edit:.3} ms\n\
         commit initial: {initial:.3} ms\n\
         commit edit: {edited_ms:.3} ms\n\
         reload previous revision: {load:.3} ms\n",
        sched = scheduled.as_secs_f64() * 1_000.0,
        edit = rescheduled.as_secs_f64() * 1_000.0,
        initial = initial_commit.as_secs_f64() * 1_000.0,
        edited_ms = edit_commit.as_secs_f64() * 1_000.0,
        load = loaded_in.as_secs_f64() * 1_000.0,
    );
    let report_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/spike-s4.txt");
    let _ = std::fs::create_dir_all(report_path.parent().unwrap());
    std::fs::write(&report_path, &report).expect("write report");
    println!("{report}");

    assert!(
        scheduled < Duration::from_millis(16),
        "initial schedule took {scheduled:?}"
    );
    assert!(
        rescheduled < Duration::from_millis(16),
        "edit schedule took {rescheduled:?}"
    );
    assert!(
        loaded_in < Duration::from_secs(1),
        "reload took {loaded_in:?}"
    );
    let _ = std::fs::remove_dir_all(directory);
}
