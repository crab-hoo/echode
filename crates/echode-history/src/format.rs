//! Stable JSON for project files.
//!
//! Keys stay in a fixed order and each note is one line, so a later git diff
//! can show a musical edit as a few changed lines. The same value always
//! produces the same bytes.

use crate::project::{Note, Project, Track};
use std::fmt::Write as _;

pub fn project_json(project: &Project) -> String {
    let mut out = String::from("{\n  \"schema\": 1,\n  \"tracks\": [\n");
    for (index, track) in project.tracks.iter().enumerate() {
        let comma = if index + 1 == project.tracks.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(out, "    \"{}\"{comma}", escape(&track.id));
    }
    out.push_str("  ]\n}\n");
    out
}

pub fn track_json(track: &Track) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\n  \"schema\": 1,\n  \"id\": \"{}\",\n  \"name\": \"{}\",\n  \"gain_milli_db\": {},\n  \"notes\": [\n",
        escape(&track.id),
        escape(&track.name),
        track.gain_milli_db
    );
    for (index, note) in track.notes.iter().enumerate() {
        let comma = if index + 1 == track.notes.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(
            out,
            "    {{\"start\":{},\"length\":{},\"pitch\":{},\"velocity\":{}}}{comma}",
            note.start, note.length, note.pitch, note.velocity
        );
    }
    out.push_str("  ]\n}\n");
    out
}

pub fn parse_project_tracks(json: &str) -> Result<Vec<String>, String> {
    let value: ProjectFile =
        serde_json::from_str(json).map_err(|error| format!("project.json: {error}"))?;
    if value.schema != 1 {
        return Err(format!("unsupported project schema {}", value.schema));
    }
    Ok(value.tracks)
}

pub fn parse_track(json: &str) -> Result<Track, String> {
    let file: TrackFile =
        serde_json::from_str(json).map_err(|error| format!("track json: {error}"))?;
    if file.schema != 1 {
        return Err(format!("unsupported track schema {}", file.schema));
    }
    Ok(Track {
        id: file.id,
        name: file.name,
        gain_milli_db: file.gain_milli_db,
        notes: file
            .notes
            .into_iter()
            .map(|note| Note {
                start: note.start,
                length: note.length,
                pitch: note.pitch,
                velocity: note.velocity,
            })
            .collect(),
    })
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

#[derive(serde::Deserialize)]
struct ProjectFile {
    schema: u32,
    tracks: Vec<String>,
}

#[derive(serde::Deserialize)]
struct TrackFile {
    schema: u32,
    id: String,
    name: String,
    gain_milli_db: i32,
    notes: Vec<NoteFile>,
}

#[derive(serde::Deserialize)]
struct NoteFile {
    start: i64,
    length: i64,
    pitch: u8,
    velocity: u8,
}
