//! Immutable control values, separate from audio-owned peak taps.
use crate::{Document, ECHODE_ERR, ECHODE_OK, Snapshot, abi};
use echode_model::{DeviceKind, ParamKey, Project};
use std::collections::BTreeMap;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MixerControls {
    pub gain_id: [u8; 16],
    pub pan_id: [u8; 16],
    pub mute_id: [u8; 16],
    pub solo_id: [u8; 16],
    pub gain: f32,
    pub pan: f32,
    pub mute: f32,
    pub solo: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ClipControls {
    pub track_id: [u8; 16],
    pub clip_id: [u8; 16],
    pub start: i64,
    pub length: i64,
    pub fade_in: i64,
    pub fade_out: i64,
    pub gain: f32,
    pub audio: u32,
    pub fade_in_ticks: i64,
    pub fade_out_ticks: i64,
}

macro_rules! clip_scalar {
    (skip, $controls:ident, $clip:ident, $field:ident) => {};
    (copy, $controls:ident, $clip:ident, $field:ident) => {
        $controls.$field = $clip.$field;
    };
    (ticks, $controls:ident, $clip:ident, $field:ident) => {
        $controls.$field = $clip.$field.get();
    };
}
macro_rules! clip_snapshot_fields {
    (
        fields { $($property:ident => $visibility:vis $field:ident: $model:ty => $type:ty,
            $value:ident, $read:ident, $encode:path, $decode:path; $ffi:ident;)+ }
        diff [$($diff:ident),+];
    ) => {
        impl ClipControls {
            fn from_placement(project: &Project, track: echode_model::Id, clip: &echode_model::Placement) -> Self {
                let mut controls = Self {
                    track_id: bytes(track), clip_id: bytes(clip.id),
                    audio: u32::from(project.content(clip.content).is_some_and(|content| content.audio.is_some())),
                    fade_in_ticks: fade_ticks(project, clip, true),
                    fade_out_ticks: fade_ticks(project, clip, false),
                    ..Self::default()
                };
                $(clip_scalar!($ffi, controls, clip, $field);)+
                controls
            }
        }
    };
}
echode_model::placement_fields!(clip_snapshot_fields);

pub(crate) struct ControlIndex {
    mixers: Vec<MixerControls>,
    clips: BTreeMap<u128, ClipControls>,
}

fn bytes(id: echode_model::Id) -> [u8; 16] {
    *uuid::Uuid::from_u128(id.raw()).as_bytes()
}

impl ControlIndex {
    pub(crate) fn build(project: &Project) -> Self {
        let mixers = project
            .tracks()
            .iter()
            .map(|track| {
                let mut controls = MixerControls::default();
                if let Some(mixer) = track
                    .devices
                    .iter()
                    .find(|device| device.kind == DeviceKind::Mixer)
                {
                    for parameter in &mixer.parameters {
                        let (id, value) = match parameter.key {
                            ParamKey::Gain => (&mut controls.gain_id, &mut controls.gain),
                            ParamKey::Pan => (&mut controls.pan_id, &mut controls.pan),
                            ParamKey::Mute => (&mut controls.mute_id, &mut controls.mute),
                            ParamKey::Solo => (&mut controls.solo_id, &mut controls.solo),
                        };
                        *id = bytes(parameter.id);
                        *value = parameter.value;
                    }
                }
                controls
            })
            .collect();
        let clips = project
            .tracks()
            .iter()
            .flat_map(|track| {
                track.clips.iter().map(move |clip| {
                    (
                        clip.id.raw(),
                        ClipControls::from_placement(project, track.id, clip),
                    )
                })
            })
            .collect();
        Self { mixers, clips }
    }
}

fn fade_ticks(project: &Project, clip: &echode_model::Placement, entering: bool) -> i64 {
    let edge = if entering {
        clip.start
    } else {
        echode_model::Tick::new(clip.start.get().saturating_add(clip.length.get()))
    };
    let Some(time) = project.tempo().flicks_at(edge) else {
        return 0;
    };
    let moved = if entering {
        time.get().saturating_add(clip.fade_in)
    } else {
        time.get().saturating_sub(clip.fade_out)
    };
    let Some(tick) = project.tempo().ticks_at(echode_model::Flick::new(moved)) else {
        return clip.length.get();
    };
    if entering {
        tick.get().saturating_sub(edge.get())
    } else {
        edge.get().saturating_sub(tick.get())
    }
    .clamp(0, clip.length.get())
}

/// # Safety
/// The snapshot is null or must remain alive. `out` is null or writable for
/// one value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_mixer_controls(
    snapshot: *const Snapshot,
    track: usize,
    out: *mut MixerControls,
) -> i32 {
    // SAFETY: the caller keeps the snapshot alive.
    let Some(value) = unsafe { abi::handle(snapshot) }.and_then(|snapshot| {
        snapshot
            .controls
            .mixers
            .get(snapshot.source_track_index(track)?)
    }) else {
        return ECHODE_ERR;
    };
    // SAFETY: the caller passes null or one writable value.
    if unsafe { abi::put(out, *value) } {
        ECHODE_OK
    } else {
        ECHODE_ERR
    }
}

/// # Safety
/// The snapshot is null or must remain alive. `clip` is null or addresses 16
/// UUID bytes, and `out` is null or writable for one value. Neither pointer
/// is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_clip_controls(
    snapshot: *const Snapshot,
    clip: *const u8,
    out: *mut ClipControls,
) -> i32 {
    // SAFETY: the caller keeps the snapshot alive and passes 16 readable bytes
    // at a non-null `clip`.
    let (Some(snapshot), Some(id)) = (unsafe { (abi::handle(snapshot), abi::uuid(clip)) }) else {
        return ECHODE_ERR;
    };
    let Some(value) = snapshot.controls.clips.get(&id) else {
        return ECHODE_ERR;
    };
    // SAFETY: the caller passes null or one writable value.
    if unsafe { abi::put(out, *value) } {
        ECHODE_OK
    } else {
        ECHODE_ERR
    }
}

/// Read a post-fader peak without submitting work to the document queue.
/// # Safety
/// The document is null, open or retained, and `track` is null or addresses
/// 16 UUID bytes. Either null reads as silence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_track_peak(
    document: *mut Document,
    track: *const u8,
) -> f32 {
    // SAFETY: the caller passes null or an open document, and 16 readable
    // bytes at a non-null `track`.
    let (Some(document), Some(id)) = (unsafe { (abi::handle(document), abi::uuid(track)) }) else {
        return 0.0;
    };
    document.service.track_peak(id)
}

/// # Safety
/// The snapshot is null or remains alive, and `out` is null or writable for
/// one i64.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_flicks(
    snapshot: *const Snapshot,
    tick: i64,
    out: *mut i64,
) -> i32 {
    // SAFETY: the caller keeps the snapshot alive.
    let Some(value) =
        unsafe { abi::handle(snapshot) }.and_then(|snapshot| snapshot.flicks_at(tick))
    else {
        return ECHODE_ERR;
    };
    // SAFETY: the caller passes null or one writable i64.
    if unsafe { abi::put(out, value) } {
        ECHODE_OK
    } else {
        ECHODE_ERR
    }
}
