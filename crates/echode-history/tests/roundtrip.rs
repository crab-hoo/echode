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

#[test]
#[ignore = "timing measurement; `make spike-history` runs it in a release build"]
fn saving_one_edited_track_of_a_thousand_touches_only_that_file() {
    use echode_history::Autosave;
    use echode_model::{
        ClipKind, Content, DEFAULT_FADE_FLICKS, Id, Lane, Note, Placement, Project,
        TICKS_PER_QUARTER, Tick,
    };

    const TRACKS: usize = 1000;
    const NOTES: i64 = 100;
    let directory = scratch();
    let mut skeleton = Project::new();
    for index in 0..TRACKS {
        let (next, _) = skeleton.add_track(format!("t{index}"));
        skeleton = next;
    }
    let note_length = TICKS_PER_QUARTER / 2;
    let mut contents = Vec::with_capacity(TRACKS);
    let mut tracks = Vec::with_capacity(TRACKS);
    for (index, track) in skeleton.tracks().iter().enumerate() {
        let mut track = (**track).clone();
        let content_id = Id::from_raw(0x10_0000 + index as u128);
        let clip_id = Id::from_raw(0x20_0000 + index as u128);
        let notes = (0..NOTES)
            .map(|note| Note {
                id: Id::from_raw(0x100_0000 + (index as u128) * 1000 + note as u128),
                start: Tick::new(note * note_length),
                length: Tick::new(note_length),
                pitch: 60,
                velocity: 100,
                release: 0,
                articulation: None,
            })
            .collect();
        contents.push(Content {
            id: content_id,
            audio: None,
            takes: Vec::new(),
            notes,
            expressions: Vec::new(),
        });
        track.clips.push(Placement {
            id: clip_id,
            content: content_id,
            lane: Lane::Timeline,
            start: Tick::new(0),
            length: Tick::new(NOTES * note_length),
            offset: Tick::new(0),
            kind: ClipKind::Fixed,
            gain: 1.0,
            fade_in: DEFAULT_FADE_FLICKS,
            fade_out: DEFAULT_FADE_FLICKS,
        });
        tracks.push(track);
    }
    let project =
        Project::try_assemble(skeleton.meter(), skeleton.tempo().clone(), contents, tracks)
            .expect("assemble");
    let autosave = Autosave::create(&directory).unwrap();
    autosave
        .save(std::sync::Arc::new(project.clone()), 0)
        .unwrap()
        .wait()
        .unwrap();

    let edited_index = 500;
    let edited_id = project.tracks()[edited_index].id;
    let neighbor_id = project.tracks()[0].id;
    let content_id = project.tracks()[0].clips[0].content;
    let mtime = |path: &std::path::Path| std::fs::metadata(path).unwrap().modified().unwrap();
    let track_path = |id: Id| directory.join("tracks").join(format!("{id}.json"));
    let content_path = directory
        .join("contents")
        .join(format!("{content_id}.json"));
    let before_edited = mtime(&track_path(edited_id));
    let before_neighbor = mtime(&track_path(neighbor_id));
    let before_content = mtime(&content_path);
    let before_project = mtime(&directory.join("project.json"));
    std::thread::sleep(Duration::from_millis(20));

    let gain = project.tracks()[edited_index].devices[0].parameters[0].id;
    let edited = project.set_parameter(gain, 0.5).unwrap();
    let started = Instant::now();
    let pending = autosave
        .save(std::sync::Arc::new(edited), 1)
        .expect("schedule");
    let scheduled = started.elapsed();
    let commit_started = Instant::now();
    pending.wait().expect("commit");
    let commit = commit_started.elapsed();

    assert_ne!(mtime(&track_path(edited_id)), before_edited);
    assert_eq!(mtime(&track_path(neighbor_id)), before_neighbor);
    assert_eq!(mtime(&content_path), before_content);
    assert_eq!(mtime(&directory.join("project.json")), before_project);
    println!(
        "product tracks: {TRACKS}\n\
         product schedule edit: {:.3} ms\n\
         product commit edit: {:.3} ms\n",
        scheduled.as_secs_f64() * 1_000.0,
        commit.as_secs_f64() * 1_000.0,
    );
    assert!(
        scheduled < Duration::from_millis(16),
        "edit schedule took {scheduled:?}"
    );
    // HIS-2 syncs `save.pending` and the commit chain on every save. Rewriting
    // every track file was 147 ms; this bound catches that regression.
    assert!(
        commit < Duration::from_millis(50),
        "edit commit took {commit:?}"
    );
    drop(autosave);
    let _ = std::fs::remove_dir_all(directory);
}
