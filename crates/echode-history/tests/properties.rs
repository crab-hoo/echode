//! Random action sequences against the model, the session, the journal, and the package.
//!
//! After every step the project keeps `check_invariants`, and a refused edit leaves it
//! unchanged. At the end the journal rebuilds the project, the package round-trips it,
//! undoing everything returns to the start, and redoing as many steps returns to the end
//! (ARCHITECTURE §16), including folder and device order. Sequence lengths and
//! case counts are this test's budget, not limits.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_actions::{ActionError, Command, Session, decode, encode, replay};
use echode_history::{read_package, write_package};
use echode_model::{CLIP_GAIN_MAX, Id, Meter, NoteDraft, Project, TICKS_PER_QUARTER, Tempo, Tick};
use proptest::prelude::*;

const EIGHTH: i64 = TICKS_PER_QUARTER / 2;
const PLUGIN_STATE: &[u8] = b"echode-rea-state";

fn plugin_state_hash() -> &'static str {
    static HASH: LazyLock<String> = LazyLock::new(|| {
        let directory =
            std::env::temp_dir().join(format!("echode-state-hash-{}", std::process::id()));
        let hash = echode_history::write_object(&directory, PLUGIN_STATE).expect("state object");
        let _ = std::fs::remove_dir_all(directory);
        hash
    });
    HASH.as_str()
}

#[derive(Clone, Debug)]
enum Step {
    AddTrack,
    InsertTrack {
        after: usize,
        into_folder: bool,
    },
    DuplicateTrack {
        track: usize,
    },
    AddChild {
        parent: usize,
    },
    SetParent {
        track: usize,
        parent: Option<usize>,
    },
    InsertNote {
        track: usize,
        start: i64,
        length: i64,
        pitch: u8,
        velocity: u8,
    },
    DeleteNote {
        track: usize,
        note: usize,
    },
    EditNote {
        track: usize,
        clip: usize,
        note: usize,
        start: i64,
        length: i64,
        pitch: u8,
    },
    GrowNote {
        track: usize,
        clip: usize,
        left: bool,
        auto: bool,
    },
    ResizeClip {
        track: usize,
        clip: usize,
        start: i64,
        length: i64,
    },
    LinkClip {
        source_track: usize,
        source_clip: usize,
        dest: usize,
        start: i64,
    },
    MoveClip {
        source_track: usize,
        source_clip: usize,
        dest: usize,
        start: i64,
        copy: bool,
    },
    SetClipGain {
        track: usize,
        clip: usize,
        gain: f32,
    },
    SetClipFades {
        track: usize,
        clip: usize,
        fade_in: i64,
        fade_out: i64,
    },
    SetParameter {
        track: usize,
        parameter: usize,
        value: f32,
    },
    SetTempo {
        at: i64,
        bpm: u32,
    },
    SetMeter {
        numerator: u32,
        denominator: u32,
    },
    AddDevice {
        track: usize,
        plugin: u8,
    },
    BypassDevice {
        track: usize,
        device: usize,
        bypassed: bool,
    },
    OfflineDevice {
        track: usize,
        device: usize,
        offline: bool,
    },
    PinParameter {
        track: usize,
        device: usize,
        parameter: u32,
        pinned: bool,
    },
    ReorderDevice {
        track: usize,
        device: usize,
        before: Option<usize>,
    },
    CopyDevice {
        source_track: usize,
        device: usize,
        target: usize,
        before: Option<usize>,
    },
    RemoveDevice {
        track: usize,
        device: usize,
    },
    SetPluginState {
        device: usize,
        present: bool,
    },
    LinkNearEnd,
    StretchSharedNote,
    PlaceAudio {
        track: usize,
        start: i64,
    },
    Undo,
    Redo,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        2 => Just(Step::AddTrack),
        2 => (any::<usize>(), any::<bool>()).prop_map(|(after, into_folder)| Step::InsertTrack { after, into_folder }),
        1 => any::<usize>().prop_map(|track| Step::DuplicateTrack { track }),
        1 => any::<usize>().prop_map(|parent| Step::AddChild { parent }),
        1 => (any::<usize>(), proptest::option::of(any::<usize>()))
            .prop_map(|(track, parent)| Step::SetParent { track, parent }),
        6 => (any::<usize>(), -16i64..96, 1i64..24, 0u8..=127, 0u8..=127).prop_map(
            |(track, start, length, pitch, velocity)| Step::InsertNote {
                track,
                start,
                length,
                pitch,
                velocity,
            }
        ),
        2 => (any::<usize>(), any::<usize>()).prop_map(|(track, note)| Step::DeleteNote { track, note }),
        4 => (any::<usize>(), any::<usize>(), any::<usize>(), -16i64..96, 1i64..24, 0u8..=127)
            .prop_map(|(track, clip, note, start, length, pitch)| Step::EditNote { track, clip, note, start, length, pitch }),
        2 => (any::<usize>(), any::<usize>(), any::<bool>(), any::<bool>())
            .prop_map(|(track, clip, left, auto)| Step::GrowNote { track, clip, left, auto }),
        2 => (any::<usize>(), any::<usize>(), -16i64..96, 1i64..48).prop_map(
            |(track, clip, start, length)| Step::ResizeClip {
                track,
                clip,
                start,
                length,
            }
        ),
        1 => (any::<usize>(), any::<usize>(), any::<usize>(), -16i64..96).prop_map(
            |(source_track, source_clip, dest, start)| Step::LinkClip {
                source_track,
                source_clip,
                dest,
                start,
            }
        ),
        // Sub-bar moves preserve auto; landings still trim and split other windows.
        2 => (any::<usize>(), any::<usize>(), any::<usize>(), -2i64..96, any::<bool>()).prop_map(
            |(source_track, source_clip, dest, start, copy)| Step::MoveClip {
                source_track,
                source_clip,
                dest,
                start,
                copy,
            }
        ),
        1 => (any::<usize>(), any::<usize>(), prop_oneof![Just(0.0f32), Just(1.0), Just(0.25), Just(-1.0), Just(CLIP_GAIN_MAX), Just(f32::MAX)])
            .prop_map(|(track, clip, gain)| Step::SetClipGain { track, clip, gain }),
        1 => (
            any::<usize>(),
            any::<usize>(),
            prop_oneof![Just(0i64), Just(3_528_000), Just(-1), Just(fade_past_u32())],
            prop_oneof![Just(0i64), Just(3_528_000), Just(-1), Just(fade_past_u32())],
        )
            .prop_map(|(track, clip, fade_in, fade_out)| Step::SetClipFades {
                track,
                clip,
                fade_in,
                fade_out,
            }),
        1 => (
            any::<usize>(),
            any::<usize>(),
            prop_oneof![Just(0.0f32), Just(1.0), Just(4.0), Just(-1.0)],
        )
            .prop_map(|(track, parameter, value)| Step::SetParameter {
                track,
                parameter,
                value,
            }),
        1 => (0i64..32, 40u32..=240).prop_map(|(at, bpm)| Step::SetTempo { at, bpm }),
        1 => (1u32..=7, prop_oneof![Just(2u32), Just(4), Just(8)])
            .prop_map(|(numerator, denominator)| Step::SetMeter { numerator, denominator }),
        2 => (any::<usize>(), 0u8..4).prop_map(|(track, plugin)| Step::AddDevice { track, plugin }),
        2 => (any::<usize>(), any::<usize>(), any::<bool>())
            .prop_map(|(track, device, bypassed)| Step::BypassDevice { track, device, bypassed }),
        2 => (any::<usize>(), any::<usize>(), any::<bool>())
            .prop_map(|(track, device, offline)| Step::OfflineDevice { track, device, offline }),
        // A few ids repeat often enough to pin, unpin, and refuse CLAP's invalid id.
        2 => (any::<usize>(), any::<usize>(), prop_oneof![Just(0u32), Just(1), Just(7), Just(u32::MAX)], any::<bool>())
            .prop_map(|(track, device, parameter, pinned)| Step::PinParameter { track, device, parameter, pinned }),
        2 => (any::<usize>(), any::<usize>(), proptest::option::of(any::<usize>()))
            .prop_map(|(track, device, before)| Step::ReorderDevice { track, device, before }),
        2 => (any::<usize>(), any::<usize>(), any::<usize>(), proptest::option::of(any::<usize>()))
            .prop_map(|(source_track, device, target, before)| Step::CopyDevice { source_track, device, target, before }),
        1 => (any::<usize>(), any::<usize>())
            .prop_map(|(track, device)| Step::RemoveDevice { track, device }),
        1 => (any::<usize>(), any::<bool>()).prop_map(|(device, present)| Step::SetPluginState {
            device,
            present,
        }),
        1 => Just(Step::LinkNearEnd),
        1 => Just(Step::StretchSharedNote),
        2 => (any::<usize>(), 0i64..16).prop_map(|(track, start)| Step::PlaceAudio { track, start }),
        2 => Just(Step::Undo),
        1 => Just(Step::Redo),
    ]
}

fn pick(ids: &[Id], index: usize) -> Option<Id> {
    (!ids.is_empty()).then(|| ids[index % ids.len()])
}

/// Turns a step into a command against the current project, or `None` when it has no target.
fn command(project: &Project, step: &Step) -> Option<Command> {
    let tracks: Vec<Id> = project.tracks().iter().map(|track| track.id).collect();
    let track = |index: usize| pick(&tracks, index);
    Some(match step {
        Step::AddTrack => Command::AddTrack {
            name: format!("track {}", tracks.len()),
        },
        Step::InsertTrack { after, into_folder } => Command::InsertTrack {
            after: track(*after)?,
            into_folder: *into_folder,
            name: format!("inserted {}", tracks.len()),
        },
        Step::DuplicateTrack { track: index } => Command::DuplicateTrack {
            source: track(*index)?,
        },
        Step::AddChild { parent } => Command::AddChild {
            parent: track(*parent)?,
            name: format!("child {}", tracks.len()),
        },
        Step::SetParent {
            track: index,
            parent,
        } => Command::SetParent {
            track: track(*index)?,
            parent: match parent {
                Some(parent) => Some(track(*parent)?),
                None => None,
            },
        },
        Step::InsertNote {
            track: index,
            start,
            length,
            pitch,
            velocity,
        } => Command::InsertNote {
            track: track(*index)?,
            draft: NoteDraft {
                pitch: *pitch,
                velocity: *velocity,
                start: Tick::new(start * EIGHTH),
                length: Tick::new(length * EIGHTH),
            },
        },
        Step::DeleteNote { track: index, note } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let notes: Vec<Id> = owner
                .clips
                .iter()
                .filter_map(|clip| project.content(clip.content))
                .flat_map(|content| content.notes.iter().map(|note| note.id))
                .collect();
            Command::DeleteNote {
                track: id,
                note: pick(&notes, *note)?,
            }
        }
        Step::EditNote {
            track: index,
            clip,
            note,
            start,
            length,
            pitch,
        } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let placement = owner.clips.get(*clip % owner.clips.len().max(1))?;
            let content = project.content(placement.content)?;
            let target = content.notes.get(*note % content.notes.len().max(1))?;
            Command::EditNote {
                track: id,
                clip: placement.id,
                note: target.id,
                pitch: *pitch,
                start: Tick::new(start * EIGHTH),
                length: Tick::new(length * EIGHTH),
            }
        }
        Step::GrowNote {
            track: index,
            clip,
            left,
            auto,
        } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let kind = if *auto {
                echode_model::ClipKind::Auto
            } else {
                echode_model::ClipKind::Fixed
            };
            let candidates: Vec<_> = owner
                .clips
                .iter()
                .filter(|clip| clip.kind == kind)
                .collect();
            let placement = candidates.get(*clip % candidates.len().max(1))?;
            let note = project.content(placement.content)?.notes.first()?;
            let old_start = placement.sounding_start(note)?.get();
            let old_end = old_start.checked_add(note.length.get())?;
            let (start, end) = if *left {
                (placement.start.get().checked_sub(1)?, old_end)
            } else {
                (
                    old_start,
                    placement
                        .start
                        .get()
                        .checked_add(placement.length.get())?
                        .max(old_end)
                        .checked_add(1)?,
                )
            };
            Command::EditNote {
                track: id,
                clip: placement.id,
                note: note.id,
                pitch: note.pitch,
                start: Tick::new(start),
                length: Tick::new(end.checked_sub(start)?.max(1)),
            }
        }
        Step::ResizeClip {
            track: index,
            clip,
            start,
            length,
        } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let clips: Vec<Id> = owner.clips.iter().map(|clip| clip.id).collect();
            Command::ResizeClip {
                track: id,
                clip: pick(&clips, *clip)?,
                start: Tick::new(start * EIGHTH),
                length: Tick::new(length * EIGHTH),
            }
        }
        Step::LinkClip {
            source_track,
            source_clip,
            dest,
            start,
        } => {
            let source_track = track(*source_track)?;
            let owner = project
                .tracks()
                .iter()
                .find(|track| track.id == source_track)?;
            let clips: Vec<Id> = owner.clips.iter().map(|clip| clip.id).collect();
            Command::LinkClip {
                source: pick(&clips, *source_clip)?,
                track: track(*dest)?,
                start: Tick::new(start * EIGHTH),
            }
        }
        Step::MoveClip {
            source_track,
            source_clip,
            dest,
            start,
            copy,
        } => {
            let source_track = track(*source_track)?;
            let owner = project
                .tracks()
                .iter()
                .find(|track| track.id == source_track)?;
            let clips: Vec<Id> = owner.clips.iter().map(|clip| clip.id).collect();
            let (clip, track, start) = (
                pick(&clips, *source_clip)?,
                track(*dest)?,
                Tick::new(start * EIGHTH),
            );
            if *copy {
                Command::DuplicateClip { clip, track, start }
            } else {
                Command::MoveClip { clip, track, start }
            }
        }
        Step::SetClipGain {
            track: index,
            clip,
            gain,
        } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let clips: Vec<Id> = owner.clips.iter().map(|clip| clip.id).collect();
            Command::SetClipGain {
                clip: pick(&clips, *clip)?,
                gain: *gain,
            }
        }
        Step::SetClipFades {
            track: index,
            clip,
            fade_in,
            fade_out,
        } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let clips: Vec<Id> = owner.clips.iter().map(|clip| clip.id).collect();
            Command::SetClipFades {
                clip: pick(&clips, *clip)?,
                fade_in: *fade_in,
                fade_out: *fade_out,
            }
        }
        Step::SetParameter {
            track: index,
            parameter,
            value,
        } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let parameters: Vec<Id> = owner
                .devices
                .iter()
                .flat_map(|device| device.parameters.iter().map(|parameter| parameter.id))
                .collect();
            Command::SetParameter {
                parameter: pick(&parameters, *parameter)?,
                value: *value,
            }
        }
        Step::SetTempo { at, bpm } => Command::SetTempo {
            at: Tick::quarters(*at).unwrap(),
            tempo: Tempo::quarters_per_minute(*bpm).unwrap(),
        },
        Step::SetMeter {
            numerator,
            denominator,
        } => Command::SetMeter {
            meter: Meter::try_new(*numerator, *denominator)?,
        },
        Step::AddDevice {
            track: index,
            plugin,
        } => {
            let plugins = [
                "dev.echode.test-gain",
                "org.example.synth",
                "org.example.synth",
                "",
            ];
            Command::AddDevice {
                track: track(*index)?,
                plugin: plugins[usize::from(*plugin)].to_string(),
            }
        }
        Step::BypassDevice {
            track: index,
            device,
            bypassed,
        } => {
            let owner = project
                .tracks()
                .iter()
                .find(|item| Some(item.id) == track(*index))?;
            let ids: Vec<_> = owner.devices.iter().map(|device| device.id).collect();
            Command::BypassDevice {
                device: pick(&ids, *device)?,
                bypassed: *bypassed,
            }
        }
        Step::OfflineDevice {
            track: index,
            device,
            offline,
        } => {
            let owner = project
                .tracks()
                .iter()
                .find(|item| Some(item.id) == track(*index))?;
            let ids: Vec<_> = owner.devices.iter().map(|device| device.id).collect();
            Command::OfflineDevice {
                device: pick(&ids, *device)?,
                offline: *offline,
            }
        }
        Step::PinParameter {
            track: index,
            device,
            parameter,
            pinned,
        } => {
            let owner = project
                .tracks()
                .iter()
                .find(|item| Some(item.id) == track(*index))?;
            let ids: Vec<_> = owner.devices.iter().map(|device| device.id).collect();
            Command::PinParameter {
                device: pick(&ids, *device)?,
                parameter: *parameter,
                pinned: *pinned,
            }
        }
        Step::ReorderDevice {
            track: index,
            device,
            before,
        } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let devices: Vec<Id> = owner.devices.iter().map(|device| device.id).collect();
            Command::ReorderDevice {
                device: pick(&devices, *device)?,
                before: before.and_then(|index| pick(&devices, index)),
            }
        }
        Step::CopyDevice {
            source_track,
            device,
            target,
            before,
        } => {
            let source_track = track(*source_track)?;
            let owner = project
                .tracks()
                .iter()
                .find(|item| item.id == source_track)?;
            let devices: Vec<Id> = owner.devices.iter().map(|device| device.id).collect();
            let target = track(*target)?;
            let anchors: Vec<Id> = project
                .tracks()
                .iter()
                .find(|item| item.id == target)?
                .devices
                .iter()
                .map(|device| device.id)
                .collect();
            Command::CopyDevice {
                source: pick(&devices, *device)?,
                track: target,
                before: before.and_then(|index| pick(&anchors, index)),
            }
        }
        Step::RemoveDevice {
            track: index,
            device,
        } => {
            let id = track(*index)?;
            let owner = project.tracks().iter().find(|track| track.id == id)?;
            let devices: Vec<Id> = owner.devices.iter().map(|device| device.id).collect();
            Command::RemoveDevice {
                device: pick(&devices, *device)?,
            }
        }
        Step::SetPluginState { device, present } => {
            let devices: Vec<Id> = project
                .tracks()
                .iter()
                .flat_map(|track| track.devices.iter())
                .filter(|device| device.plugin.is_some())
                .map(|device| device.id)
                .collect();
            Command::SetPluginState {
                device: pick(&devices, *device)?,
                state: present.then(|| plugin_state_hash().to_string()),
            }
        }
        Step::LinkNearEnd => {
            let source = project
                .tracks()
                .iter()
                .find_map(|track| track.clips.first().map(|clip| (clip.id, track.id)))?;
            Command::LinkClip {
                source: source.0,
                track: source.1,
                start: Tick::new(i64::MAX - 3),
            }
        }
        Step::StretchSharedNote => {
            let track_id = project.tracks().first()?.id;
            let clip = project.tracks()[0].clips.first()?;
            let note = project.content(clip.content)?.notes.first()?;
            Command::EditNote {
                track: track_id,
                clip: clip.id,
                note: note.id,
                pitch: note.pitch,
                start: Tick::new(clip.start.get()),
                length: Tick::new(8),
            }
        }
        Step::PlaceAudio {
            track: index,
            start,
        } => Command::PlaceAudio {
            track: track(*index)?,
            start: Tick::new(start * EIGHTH),
            hash: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            sample_rate: 48_000,
            channels: 1,
            frames: 48_000,
        },
        Step::Undo | Step::Redo => return None,
    })
}

fn fade_past_u32() -> i64 {
    let rate = i128::from(768_000);
    let frames = i128::from(u32::MAX) + 1;
    let numer = frames * 705_600_000;
    i64::try_from((numer + rate - 1) / rate).unwrap()
}

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("echode-properties-{}-{n}", std::process::id()));
    // A failed earlier run can leave a package under a reused pid. Its
    // journal would replay into this test.
    let _ = std::fs::remove_dir_all(&path);
    path
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn random_edits_keep_every_property(steps in prop::collection::vec(step(), 1..48)) {
        let base = Project::new();
        let mut session = Session::open(base.clone());
        for step in &steps {
            let before = session.project().clone();
            let result = match step {
                Step::Undo => session.undo().map(|_| ()),
                Step::Redo => session.redo().map(|_| ()),
                other => match command(session.project(), other) {
                    Some(command) => session.perform(command).map(|_| ()),
                    None => Ok(()),
                },
            };
            match result {
                Ok(()) => {
                    if !matches!(step, Step::Undo | Step::Redo) {
                        let changes = before.diff(session.project());
                        let forward = before.apply_changeset(&changes).unwrap();
                        let backward = session.project().apply_changeset(&changes.invert()).unwrap();
                        prop_assert_eq!(&forward, session.project(), "changeset for {:?}", step);
                        prop_assert_eq!(&backward, &before, "inverse changeset for {:?}", step);
                    }
                }
                Err(ActionError::Model(_) | ActionError::NothingToUndo | ActionError::NothingToRedo) => {
                    prop_assert_eq!(session.project(), &before, "a refused {:?} changed the project", step);
                }
                Err(error) => prop_assert!(false, "{:?} failed with {}", step, error),
            }
            if let Err(broken) = session.project().check_invariants() {
                prop_assert!(false, "after {:?}: {}", step, broken);
            }
        }

        let last = session.project().clone();
        prop_assert_eq!(&replay(&base, session.journal()).unwrap(), &last);
        let decoded = decode(&encode(session.journal())).unwrap();
        prop_assert_eq!(&replay(&base, &decoded).unwrap(), &last);

        let directory = scratch();
        write_package(&directory, &last).unwrap();
        let _ = echode_history::write_object(&directory, PLUGIN_STATE);
        let loaded = read_package(&directory);
        let _ = std::fs::remove_dir_all(&directory);
        prop_assert_eq!(&loaded.unwrap(), &last);

        let mut undone = 0;
        while session.undo().is_ok() {
            undone += 1;
        }
        prop_assert_eq!(session.project(), &base);
        for _ in 0..undone {
            session.redo().unwrap();
        }
        prop_assert_eq!(session.project(), &last);
    }
}
