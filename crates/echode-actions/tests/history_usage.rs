//! What undo history keeps in memory for common long sessions (ADR 0286).
//!
//! The sizes below are measurement scenarios, not product limits.

use std::time::Instant;

use echode_actions::{Command, Session};
use echode_model::{NoteDraft, Project, TICKS_PER_QUARTER, Tick};

const STEPS: usize = 1_000;

/// Prints the usage and how long the document thread spends answering it.
fn report(scenario: &str, session: &Session) {
    let started = Instant::now();
    let usage = session.usage();
    let elapsed = started.elapsed();
    let steps = usage.undo_steps.max(1);
    println!(
        "history_usage scenario={scenario} tracks={} contents={} clips={} steps={} kept_tracks={} \
         kept_contents={} kept_notes={} kept_bytes={} bytes_per_step={} journal_bytes={} \
         current_bytes={} query_ms={:.3}",
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
        usage.versions.contents,
        usage.versions.notes,
        usage.versions.bytes,
        usage.versions.bytes / steps,
        usage.journal_bytes,
        session.project().footprint().bytes,
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
    // One long clip: every velocity edit replaces the whole content.
    let mut session = Session::open(project(1, 10_000));
    let (track, clip) = {
        let track = &session.project().tracks()[0];
        (track.id, track.clips[0].id)
    };
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
    report("velocity_10000_notes", &session);

    // A mix pass over many tracks: every gain edit replaces one track.
    let mut session = Session::open(project(128, 64));
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
    report("gain_128_tracks_64_notes", &session);

    // Structure edits: each version owns its own track list.
    let mut session = Session::open(project(128, 1));
    for step in 0..STEPS {
        session
            .perform(Command::AddTrack {
                name: format!("added {step}"),
            })
            .unwrap();
    }
    report("add_track_from_128", &session);
}
