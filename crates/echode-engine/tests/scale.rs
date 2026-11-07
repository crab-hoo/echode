//! Release measurement of work that grows with the score (FCA-01 to FCA-05 in
//! docs/reviews/2026-11-03-full-code-audit.md).
//!
//! Without plugins or a device, it times the block that wraps a loop, the block
//! that adopts an edit while playing, the control thread's replacement, and one
//! model edit, load, changeset diff, and repeated track insertions. At a fixed
//! track count, a block that slows as notes are added still does work proportional
//! to the score on the audio thread. Counts and rounds bound this measurement
//! only, not the product.
//!
//! `make spike-scale` runs this and writes `target/spike-scale.json`. It is not
//! a correctness test and has no pass threshold; compare before and after a fix.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use echode_engine::{PlaybackAudio, PlaybackCommand, PlaybackOptions, playback};
use echode_model::{
    ClipKind, Content, Id, NoteDraft, NoteGeometry, Placement, Project, Tick, Track,
};

const RATE: u32 = 48_000;
const FRAMES: usize = 128;
/// About 128 samples at 120 bpm: one short note starts in each block.
const STEP: i64 = 3_763;

struct Template {
    project: Project,
    clip: Placement,
    content: Content,
}

/// `tracks` empty tracks, plus one clip and content to copy.
fn template(tracks: usize) -> Template {
    let mut project = Project::new();
    let mut ids = Vec::new();
    for index in 0..tracks {
        let (next, id) = project.add_track(format!("t{index}"));
        project = next;
        ids.push(id);
    }
    let draft = NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(0),
        length: Tick::new(1_000),
    };
    let (with_note, _) = project.insert_note(ids[0], draft).unwrap();
    let clip = with_note.tracks()[0].clips[0].clone();
    let content = (*with_note.contents()[0]).clone();
    Template {
        project,
        clip,
        content,
    }
}

/// Assembled at once, because inserting a note at a time is itself quadratic.
fn build(template: &Template, clips: usize, notes: usize, step: i64) -> Project {
    let mut contents = Vec::new();
    let mut tracks: Vec<Track> = Vec::new();
    let length = (notes as i64 + 1) * step;
    for track in template.project.tracks() {
        let mut track = (**track).clone();
        for index in 0..clips {
            let mut content = template.content.clone();
            content.id = Id::generate();
            content.notes = (0..notes)
                .map(|note| {
                    let mut copy = template.content.notes[0].clone();
                    copy.id = Id::generate();
                    copy.pitch = 60 + (note % 12) as u8;
                    copy.start = Tick::new(note as i64 * step);
                    copy.length = Tick::new(step / 2);
                    copy
                })
                .collect();
            content
                .notes
                .sort_by_key(|note| (note.start, note.pitch, note.id));
            let mut clip = template.clip.clone();
            clip.id = Id::generate();
            clip.content = content.id;
            clip.kind = ClipKind::Fixed;
            clip.start = Tick::new(index as i64 * length);
            clip.offset = Tick::new(0);
            clip.length = Tick::new(length);
            track.clips.push(clip);
            contents.push(content);
        }
        tracks.push(track);
    }
    let meter = template.project.meter();
    let tempo = template.project.tempo().clone();
    Project::try_assemble(meter, tempo, contents, tracks).unwrap()
}

fn block(audio: &mut PlaybackAudio, left: &mut [f32], right: &mut [f32]) -> Duration {
    let started = Instant::now();
    audio.render(left, right);
    started.elapsed()
}

fn us(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn percentile(sorted: &[Duration], share: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * share).round() as usize]
}

/// Moves the last note of track 0, a change no block will reach.
fn edited(project: &Project, round: u32) -> Project {
    let track = &project.tracks()[0];
    let clip = &track.clips[0];
    let note = project.content(clip.content).unwrap().notes.last().unwrap();
    let geometry = NoteGeometry {
        pitch: 61 + (round % 2) as u8,
        start: note.start,
        length: note.length,
    };
    project
        .edit_note(track.id, clip.id, note.id, geometry)
        .unwrap()
}

/// A 32-block loop near the end of the notes. Every wrap follows all of them.
fn loop_wrap(tracks: usize, notes: usize) -> String {
    let project = build(&template(tracks), 1, notes, STEP);
    let (mut control, mut audio) =
        playback(&project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    let end = (notes as i64 - 8) * FRAMES as i64;
    let start = end - 32 * FRAMES as i64 - 37;
    control.command(PlaybackCommand::Locate(start)).unwrap();
    control
        .command(PlaybackCommand::SetLoop { start, end })
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; FRAMES], vec![0.0; FRAMES]);
    let (mut wraps, mut others) = (Vec::new(), Vec::new());
    let mut last = control.status().sample;
    for _ in 0..400 {
        let took = block(&mut audio, &mut left, &mut right);
        control.service();
        let now = control.status().sample;
        if now < last {
            wraps.push(took);
        } else {
            others.push(took);
        }
        last = now;
    }
    wraps.sort();
    others.sort();
    let row = format!(
        "{{\"case\": \"loop_wrap\", \"tracks\": {tracks}, \"notes_per_track\": {notes}, \
         \"other_p50_us\": {:.1}, \"wraps\": {}, \"wrap_p50_us\": {:.1}, \"wrap_max_us\": {:.1}}}",
        us(percentile(&others, 0.5)),
        wraps.len(),
        us(percentile(&wraps, 0.5)),
        us(*wraps.last().unwrap()),
    );
    println!("{row}");
    row
}

/// Edits while playing, after `played` blocks since the last locate.
fn edit_adoption(tracks: usize, notes: usize, played: usize) -> String {
    let project = build(&template(tracks), 1, notes, STEP);
    let (mut control, mut audio) =
        playback(&project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; FRAMES], vec![0.0; FRAMES]);
    let mut normal = Vec::new();
    for _ in 0..played {
        normal.push(block(&mut audio, &mut left, &mut right));
        control.service();
    }
    let (mut adoptions, mut replacements) = (Vec::new(), Vec::new());
    for round in 0..20 {
        let next = edited(&project, round);
        let started = Instant::now();
        control.replace(&next).unwrap();
        replacements.push(started.elapsed());
        adoptions.push(block(&mut audio, &mut left, &mut right));
        control.service();
    }
    normal.sort();
    adoptions.sort();
    replacements.sort();
    let row = format!(
        "{{\"case\": \"edit_adoption\", \"tracks\": {tracks}, \"notes_per_track\": {notes}, \
         \"played_blocks\": {played}, \"normal_p50_us\": {:.1}, \"adoption_p50_us\": {:.1}, \
         \"adoption_max_us\": {:.1}, \"replace_p50_ms\": {:.1}, \"reserved_events\": {}}}",
        us(percentile(&normal, 0.5)),
        us(percentile(&adoptions, 0.5)),
        us(*adoptions.last().unwrap()),
        us(percentile(&replacements, 0.5)) / 1e3,
        control.reserved_events(),
    );
    println!("{row}");
    row
}

/// One load check and one note edit, with each clip on its own content.
fn model_edit(tracks: usize, clips: usize) -> String {
    let project = build(&template(tracks), clips, 8, 3_000);
    let contents: Vec<Content> = project.contents().iter().map(|c| (**c).clone()).collect();
    let parts: Vec<Track> = project.tracks().iter().map(|t| (**t).clone()).collect();
    let started = Instant::now();
    Project::try_assemble(project.meter(), project.tempo().clone(), contents, parts).unwrap();
    let load = started.elapsed();
    let track = &project.tracks()[0];
    let clip = &track.clips[0];
    let note = &project.content(clip.content).unwrap().notes[0];
    let geometry = NoteGeometry {
        pitch: note.pitch + 1,
        start: note.start,
        length: note.length,
    };
    let started = Instant::now();
    let after = project
        .edit_note(track.id, clip.id, note.id, geometry)
        .unwrap();
    let edit = started.elapsed();
    let started = Instant::now();
    let changes = project.diff(&after);
    let diff = started.elapsed();
    std::hint::black_box(changes);
    // Repeated insertions all start at the same snapshot; only the returned
    // versions are dropped. This measures control-side edits, without devices.
    let mut additions = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let added = project.add_track("scale addition");
        additions.push(started.elapsed());
        std::hint::black_box(added);
    }
    additions.sort();
    let row = format!(
        "{{\"case\": \"model_edit\", \"tracks\": {tracks}, \"clips_per_track\": {clips}, \
         \"assemble_ms\": {:.2}, \"edit_note_ms\": {:.2}, \"diff_ms\": {:.2}, \"add_track_p50_ms\": {:.2}}}",
        us(load) / 1e3,
        us(edit) / 1e3,
        us(diff) / 1e3,
        us(percentile(&additions, 0.5)) / 1e3,
    );
    println!("{row}");
    row
}

#[test]
#[ignore = "release measurement; run with make spike-scale"]
fn measure_playback_scale() {
    let mut rows = Vec::new();
    for notes in [1_000, 4_000, 16_000] {
        rows.push(loop_wrap(64, notes));
    }
    rows.push(loop_wrap(256, 4_000));
    for played in [100, 3_500] {
        rows.push(edit_adoption(256, 4_000, played));
    }
    for (tracks, clips) in [(64, 64), (128, 64)] {
        rows.push(model_edit(tracks, clips));
    }
    let report = format!(
        "{{\n  \"rate\": {RATE},\n  \"frames\": {FRAMES},\n  \"plugins\": false,\n  \"rows\": [\n    {}\n  ]\n}}\n",
        rows.join(",\n    ")
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/spike-scale.json");
    std::fs::write(&path, report).unwrap();
}
