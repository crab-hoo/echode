//! Immutable control values, separate from audio-owned peak taps.
use crate::{Document, ECHODE_ERR, ECHODE_OK, Snapshot, document_ref, snapshot_ref};
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
                        ClipControls {
                            track_id: bytes(track.id),
                            clip_id: bytes(clip.id),
                            start: clip.start.get(),
                            length: clip.length.get(),
                            fade_in: clip.fade_in,
                            fade_out: clip.fade_out,
                            gain: clip.gain,
                            audio: u32::from(
                                project
                                    .content(clip.content)
                                    .is_some_and(|content| content.audio.is_some()),
                            ),
                            fade_in_ticks: fade_ticks(project, clip, true),
                            fade_out_ticks: fade_ticks(project, clip, false),
                        },
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
/// The snapshot must remain alive. `out` is null or writable for one value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_mixer_controls(
    snapshot: *const Snapshot,
    track: usize,
    out: *mut MixerControls,
) -> i32 {
    let Some(value) = snapshot_ref(snapshot).and_then(|snapshot| {
        snapshot
            .controls
            .mixers
            .get(snapshot.source_track_index(track)?)
    }) else {
        return ECHODE_ERR;
    };
    if out.is_null() {
        return ECHODE_ERR;
    }
    unsafe {
        out.write(*value);
    }
    ECHODE_OK
}

/// # Safety
/// The snapshot must remain alive. `clip` addresses 16 UUID bytes and `out`
/// is null or writable for one value. Neither pointer is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_clip_controls(
    snapshot: *const Snapshot,
    clip: *const u8,
    out: *mut ClipControls,
) -> i32 {
    if clip.is_null() || out.is_null() {
        return ECHODE_ERR;
    }
    let id = u128::from_be_bytes(unsafe { std::ptr::read_unaligned(clip.cast::<[u8; 16]>()) });
    let Some(value) = snapshot_ref(snapshot).and_then(|snapshot| snapshot.controls.clips.get(&id))
    else {
        return ECHODE_ERR;
    };
    unsafe {
        out.write(*value);
    }
    ECHODE_OK
}

/// Read a post-fader peak without submitting work to the document queue.
/// # Safety
/// The document is open or retained and `track` addresses 16 UUID bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_track_peak(
    document: *mut Document,
    track: *const u8,
) -> f32 {
    if track.is_null() {
        return 0.0;
    }
    let id = u128::from_be_bytes(unsafe { std::ptr::read_unaligned(track.cast::<[u8; 16]>()) });
    document_ref(document).map_or(0.0, |document| document.service.track_peak(id))
}

/// # Safety
/// The snapshot remains alive and `out` is null or writable for one i64.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_flicks(
    snapshot: *const Snapshot,
    tick: i64,
    out: *mut i64,
) -> i32 {
    if out.is_null() {
        return ECHODE_ERR;
    }
    let Some(value) = snapshot_ref(snapshot).and_then(|snapshot| snapshot.flicks_at(tick)) else {
        return ECHODE_ERR;
    };
    unsafe {
        out.write(value);
    }
    ECHODE_OK
}
