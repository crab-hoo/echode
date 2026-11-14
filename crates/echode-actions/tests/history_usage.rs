//! What undo history keeps in memory for common long sessions (ADR 0286).
//!
//! The sizes below are measurement scenarios, not product limits.

use std::time::{Duration, Instant};

use echode_actions::{Command, Session};
use echode_model::{Id, NoteDraft, Project, TICKS_PER_QUARTER, Tick};

const STEPS: usize = 1_000;

/// Prints the usage, the mean time of one edit, and how long the document
/// thread spends answering the usage query.
fn report(scenario: &str, session: &Session, edits: Duration) {
    let started = Instant::now();
    let usage = session.usage();
    let elapsed = started.elapsed();
    let steps = usage.undo_steps.max(1);
    println!(
        "history_usage scenario={scenario} tracks={} contents={} clips={} steps={} kept_tracks={} \
         kept_placements={} kept_contents={} kept_notes={} kept_bytes={} bytes_per_step={} \
         journal_bytes={} current_bytes={} edit_us={:.1} query_ms={:.3}",
        session.project().tracks().len(),
        session.project().contents().len(),
        session
            .project()
            .tracks()
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>(),
        usage.undo_steps,
        usage.versions.tracks,
        usage.versions.placements,
        usage.versions.contents,
        usage.versions.notes,
        usage.versions.bytes,
        usage.versions.bytes / steps,
        usage.journal_bytes,
        session.project().footprint().bytes,
        edits.as_secs_f64() * 1e6 / steps as f64,
        elapsed.as_secs_f64() * 1_000.0,
    );
}

/// `tracks` tracks, each with one clip of `notes` sixteenth notes.
fn project(tracks: usize, notes: i64) -> Project {
    let mut project = Project::new();
    for index in 0..tracks {
        let (next, track) = project.add_track(format!("track {index}"));
        project = next;
        for note in 0..notes {
            let draft = NoteDraft {
                pitch: 36 + (note % 48) as u8,
                velocity: 100,
                start: Tick::new(note * TICKS_PER_QUARTER / 4),
                length: Tick::new(TICKS_PER_QUARTER / 4),
            };
            project = project.insert_note(track, draft).unwrap().0;
        }
    }
    project
}

#[test]
#[ignore = "memory measurement; `make spike-history` runs it"]
fn measure_history_usage() {
    // One long clip: a velocity edit replaces the run that holds the note
    // (ADR 0287). Before the runs, it replaced the whole content.
    let mut session = Session::open(project(1, 10_000));
    let (track, clip) = {
        let track = &session.project().tracks()[0];
        (track.id, track.clips[0].id)
    };
    let started = Instant::now();
    for step in 0..STEPS {
        let note = session.project().contents()[0].notes[step].id;
        session
            .perform(Command::SetNoteVelocity {
                track,
                clip,
                note,
                // 1..=99 never repeats the fixture's 100, so every step records.
                velocity: 1 + (step % 99) as u8,
            })
            .unwrap();
    }
    report("velocity_10000_notes", &session, started.elapsed());

    // The same clip, moving notes in time and pitch: each move leaves one run
    // and enters another.
    let mut session = Session::open(project(1, 10_000));
    let (track, clip) = {
        let track = &session.project().tracks()[0];
        (track.id, track.clips[0].id)
    };
    let started = Instant::now();
    for step in 0..STEPS {
        let project = session.project();
        let note = &project.contents()[0].notes[(step * 7_919) % 10_000];
        let (note, pitch, start, length) = (note.id, note.pitch, note.start, note.length);
        session
            .perform(Command::EditNote {
                track,
                clip,
                note,
                pitch: 36 + (pitch - 36 + 1) % 48,
                start: Tick::new(
                    (start.get() + TICKS_PER_QUARTER * 4) % (10_000 * TICKS_PER_QUARTER / 4),
                ),
                length,
            })
            .unwrap();
    }
    report("move_10000_notes", &session, started.elapsed());

    // A mix pass over many tracks: every gain edit replaces one track.
    let mut session = Session::open(project(128, 64));
    let started = Instant::now();
    for step in 0..STEPS {
        let track = &session.project().tracks()[step % 128];
        let gain = track.devices[0].parameters[0].id;
        session
            .perform(Command::SetParameter {
                parameter: gain,
                value: (step % 100) as f32 / 100.0,
            })
            .unwrap();
    }
    report("gain_128_tracks_64_notes", &session, started.elapsed());

    // Structure edits: each version owns its own track list.
    let mut session = Session::open(project(128, 1));
    let started = Instant::now();
    for step in 0..STEPS {
        session
            .perform(Command::AddTrack {
                name: format!("added {step}"),
            })
            .unwrap();
    }
    report("add_track_from_128", &session, started.elapsed());

    // A sliced part: one track of many short clips, each with its own content.
    // A clip edit replaces the run holding the placement (ADR 0353); before the
    // runs it replaced the whole track. Every version owns the content list.
    let mut session = Session::open(clips(1, 2_000));
    let started = Instant::now();
    for step in 0..STEPS {
        let track = &session.project().tracks()[0];
        let clip = &track.clips[(step * 7_919) % track.clips.len()];
        let bar = TICKS_PER_QUARTER * 4;
        let length = if clip.length.get() == bar {
            bar / 2
        } else {
            bar
        };
        session
            .perform(Command::ResizeClip {
                track: track.id,
                clip: clip.id,
                start: clip.start,
                length: Tick::new(length),
            })
            .unwrap();
    }
    report("resize_clip_2000_clips", &session, started.elapsed());

    // A long arrangement: a mix edit holds one short track, but every version
    // owns the lists of all tracks and contents.
    let mut session = Session::open(clips(64, 160));
    let started = Instant::now();
    for step in 0..STEPS {
        let track = &session.project().tracks()[step % 64];
        let gain = track.devices[0].parameters[0].id;
        session
            .perform(Command::SetParameter {
                parameter: gain,
                value: (step % 100) as f32 / 100.0,
            })
            .unwrap();
    }
    report("gain_64_tracks_160_clips", &session, started.elapsed());
}

/// `tracks` tracks, each with `per_track` one-bar clips two bars apart, every
/// clip with its own one-note content.
fn clips(tracks: usize, per_track: i64) -> Project {
    let bar = TICKS_PER_QUARTER * 4;
    let mut project = project(tracks, 1);
    for index in 0..tracks {
        let track = project.tracks()[index].id;
        let first = project.tracks()[index].clips[0].id;
        for slot in 1..per_track {
            project = project
                .duplicate_clip(first, track, Tick::new(slot * 2 * bar), Id::generate())
                .unwrap();
        }
        assert_eq!(project.tracks()[index].clips.len() as i64, per_track);
    }
    project
}
