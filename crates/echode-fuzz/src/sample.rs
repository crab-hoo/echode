//! A small saved package built by the product: tracks, a folder, notes, tempo
//! and meter changes, an imported audio clip with gain and fades, a plugin
//! with stored state, and mixer parameters. Seeds for the package and journal
//! targets start from it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use echode_actions::{Command, JournalLine, JournalOp, Session, encode_line};
use echode_history::{StoredAudio, import_audio, write_object, write_package};
use echode_model::{Id, Meter, NoteDraft, Project, TICKS_PER_QUARTER, Tempo, Tick};

const QUARTER: i64 = TICKS_PER_QUARTER;

/// The sample package as an archive (see `package.rs`) and the journal that
/// continues it, with every id replaced by a fixed one.
pub(crate) struct Sample {
    pub(crate) archive: Vec<u8>,
    pub(crate) journal: String,
}

/// Ids are time-based, so two processes would build different packages. The
/// fixed ids keep the order of the originals, which is the order the edits
/// created them, so a journal seed written by one process replays over the
/// base package another process writes.
pub(crate) fn sample() -> &'static Sample {
    static SAMPLE: OnceLock<Sample> = OnceLock::new();
    SAMPLE.get_or_init(|| {
        let scratch = crate::scratch::Scratch::new("sample");
        let (_, lines) = write(scratch.path());
        let archive = crate::package::archive(scratch.path(), None);
        let journal = journal_text(&lines);
        let ids = fixed_ids([archive.as_slice(), journal.as_bytes()]);
        Sample {
            archive: replace_ids(&archive, &ids),
            journal: String::from_utf8(replace_ids(journal.as_bytes(), &ids))
                .expect("ids are ASCII"),
        }
    })
}

const ID_LEN: usize = 36;

fn is_id(window: &[u8]) -> bool {
    window.len() == ID_LEN
        && window.iter().enumerate().all(|(index, &byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn fixed_ids<'a>(texts: impl IntoIterator<Item = &'a [u8]>) -> BTreeMap<u128, String> {
    let mut originals = BTreeMap::new();
    for text in texts {
        for window in text.windows(ID_LEN).filter(|window| is_id(window)) {
            let id = Id::parse(std::str::from_utf8(window).expect("ASCII")).expect("an id");
            originals.insert(id.raw(), String::new());
        }
    }
    for (index, fixed) in originals.values_mut().enumerate() {
        *fixed = format!("0190a000-0000-7000-8000-{index:012x}");
    }
    originals
}

fn replace_ids(text: &[u8], ids: &BTreeMap<u128, String>) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if let Some(window) = text.get(at..at + ID_LEN).filter(|window| is_id(window)) {
            let id = Id::parse(std::str::from_utf8(window).expect("ASCII")).expect("an id");
            out.extend_from_slice(ids[&id.raw()].as_bytes());
            at += ID_LEN;
        } else {
            out.push(text[at]);
            at += 1;
        }
    }
    out
}

/// Writes the package to `package` and returns the saved project and journal
/// lines that continue it: edits, an undo and a redo.
fn write(package: &Path) -> (Project, Vec<JournalLine>) {
    let audio = import_sample_audio(package);
    let state = write_object(package, b"recorded plugin state").expect("store a plugin state");
    let mut session = Session::open(Project::new());
    // The meter cannot change once a track has clips.
    perform(
        &mut session,
        Command::SetMeter {
            meter: Meter::try_new(3, 4).expect("3/4"),
        },
    );
    let lead = add_track(&mut session, "Lead");
    let drums = add_track(&mut session, "Drums");
    let folder = add_track(&mut session, "Folder");
    perform(
        &mut session,
        Command::AddChild {
            parent: folder,
            name: "Child".to_string(),
        },
    );
    for (index, pitch) in [60u8, 64, 67].into_iter().enumerate() {
        perform(
            &mut session,
            Command::InsertNote {
                track: lead,
                draft: NoteDraft {
                    pitch,
                    velocity: 90 + index as u8,
                    start: Tick::new(index as i64 * QUARTER),
                    length: Tick::new(QUARTER / 2),
                },
            },
        );
    }
    perform(
        &mut session,
        Command::SetTempo {
            at: Tick::new(8 * QUARTER),
            tempo: Tempo::quarters_per_minute(90).expect("90 BPM"),
        },
    );
    perform(
        &mut session,
        Command::PlaceAudio {
            track: drums,
            start: Tick::new(QUARTER),
            hash: audio.hash.clone(),
            sample_rate: audio.sample_rate,
            channels: audio.channels,
            frames: audio.frames,
        },
    );
    let clip = last_clip(session.project(), drums);
    perform(&mut session, Command::SetClipGain { clip, gain: 0.5 });
    perform(
        &mut session,
        Command::SetClipFades {
            clip,
            fade_in: 1_000,
            fade_out: 2_000,
        },
    );
    perform(
        &mut session,
        Command::AddDevice {
            track: lead,
            plugin: "org.example.synth".to_string(),
        },
    );
    let device = plugin_device(session.project(), lead);
    perform(
        &mut session,
        Command::SetPluginState {
            device,
            state: Some(state),
        },
    );
    if let Some(parameter) = first_parameter(session.project(), drums) {
        perform(
            &mut session,
            Command::SetParameter {
                parameter,
                value: 0.25,
            },
        );
    }
    let project = session.project().clone();
    write_package(package, &project).expect("write the sample package");

    let saved = session.journal().len();
    let note_clip = last_clip(session.project(), lead);
    perform(
        &mut session,
        Command::LinkClip {
            source: note_clip,
            track: lead,
            start: Tick::new(16 * QUARTER),
        },
    );
    perform(
        &mut session,
        Command::MoveClip {
            clip,
            track: drums,
            start: Tick::new(2 * QUARTER),
        },
    );
    perform(&mut session, Command::DuplicateTrack { source: lead });
    perform(
        &mut session,
        Command::BypassDevice {
            device,
            bypassed: true,
        },
    );
    let mut lines: Vec<JournalLine> = session.journal()[saved..]
        .iter()
        .enumerate()
        .map(|(index, entry)| JournalLine {
            seq: index as u64 + 1,
            op: JournalOp::Edit,
            actions: entry.actions.clone(),
            changes: entry.changes.clone().expect("an edit has changes"),
        })
        .collect();
    let last = lines.last().expect("journal lines").clone();
    let seq = last.seq;
    lines.push(JournalLine {
        seq: seq + 1,
        op: JournalOp::Undo,
        ..last.clone()
    });
    lines.push(JournalLine {
        seq: seq + 2,
        op: JournalOp::Redo,
        ..last
    });
    (project, lines)
}

fn journal_text(lines: &[JournalLine]) -> String {
    lines.iter().map(|line| encode_line(line) + "\n").collect()
}

fn import_sample_audio(package: &Path) -> StoredAudio {
    let source = package.with_extension("source.wav");
    std::fs::create_dir_all(source.parent().expect("a parent")).expect("create the parent");
    std::fs::write(&source, wav_pcm16(48_000, 2, 480)).expect("write the sample audio");
    let stored = import_audio(package, &source).expect("import the sample audio");
    let _ = std::fs::remove_file(&source);
    stored
}

/// A 16-bit PCM WAV file with a short ramp in each channel.
pub(crate) fn wav_pcm16(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
    let block = u32::from(channels) * 2;
    let data = frames * block;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * block).to_le_bytes());
    bytes.extend_from_slice(&(block as u16).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for frame in 0..frames {
        for channel in 0..channels {
            let value = (frame as i32 * 64 - 8_000 * i32::from(channel)) as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

/// A 32-bit float WAV file (`WAVE_FORMAT_IEEE_FLOAT`).
pub(crate) fn wav_float(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
    let block = u32::from(channels) * 4;
    let data = frames * block;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * block).to_le_bytes());
    bytes.extend_from_slice(&(block as u16).to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for frame in 0..frames {
        for channel in 0..channels {
            let value = frame as f32 / frames as f32 - 0.5 * f32::from(channel);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

/// A 16-bit AIFF file at 48 kHz.
pub(crate) fn aiff_pcm16(channels: u16, frames: u32) -> Vec<u8> {
    let data = frames * u32::from(channels) * 2;
    let mut bytes = Vec::with_capacity(54 + data as usize);
    bytes.extend_from_slice(b"FORM");
    bytes.extend_from_slice(&(46 + data).to_be_bytes());
    bytes.extend_from_slice(b"AIFFCOMM");
    bytes.extend_from_slice(&18u32.to_be_bytes());
    bytes.extend_from_slice(&channels.to_be_bytes());
    bytes.extend_from_slice(&frames.to_be_bytes());
    bytes.extend_from_slice(&16u16.to_be_bytes());
    // 48,000 as an 80-bit extended float.
    bytes.extend_from_slice(&[0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]);
    bytes.extend_from_slice(b"SSND");
    bytes.extend_from_slice(&(8 + data).to_be_bytes());
    bytes.extend_from_slice(&[0; 8]);
    for frame in 0..frames {
        for channel in 0..channels {
            let value = (frame as i32 * 32 + 4_000 * i32::from(channel)) as i16;
            bytes.extend_from_slice(&value.to_be_bytes());
        }
    }
    bytes
}

fn add_track(session: &mut Session, name: &str) -> Id {
    perform(
        session,
        Command::AddTrack {
            name: name.to_string(),
        },
    );
    session
        .project()
        .tracks()
        .last()
        .expect("the added track")
        .id
}

fn last_clip(project: &Project, track: Id) -> Id {
    track_of(project, track)
        .clips
        .last()
        .expect("a clip on the track")
        .id
}

fn plugin_device(project: &Project, track: Id) -> Id {
    track_of(project, track)
        .devices
        .iter()
        .find(|device| device.plugin.is_some())
        .expect("a plugin on the track")
        .id
}

fn first_parameter(project: &Project, track: Id) -> Option<Id> {
    track_of(project, track)
        .devices
        .iter()
        .flat_map(|device| &device.parameters)
        .map(|parameter| parameter.id)
        .next()
}

fn track_of(project: &Project, track: Id) -> &echode_model::Track {
    project
        .tracks()
        .iter()
        .find(|candidate| candidate.id == track)
        .expect("the track")
}

fn perform(session: &mut Session, command: Command) {
    let description = format!("{command:?}");
    session
        .perform(command)
        .unwrap_or_else(|error| panic!("sample edit {description} failed: {error}"));
}
