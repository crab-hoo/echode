use crate::{PianoNoteElement, Snapshot, snapshot_ref};
use echode_draw::PianoView;
use echode_model::{Id, Tick};
use std::os::raw::c_int;

/// Writes piano notes intersecting time and pitch. Null output with capacity
/// zero counts notes in `dropped`. Invalid handles return -1 and zero counts.
///
/// # Safety
/// UUID inputs have 16 readable bytes. `snapshot` is null or owned. `out` has
/// room for `capacity` elements. Count pointers are writable and do not alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_piano_visible(
    snapshot: *const Snapshot,
    track: *const u8,
    origin_tick: i64,
    width_ticks: i64,
    top_pitch: f64,
    height_pitches: f64,
    pixels_per_tick: f64,
    pixels_per_pitch: f64,
    out: *mut PianoNoteElement,
    capacity: usize,
    written: *mut usize,
    dropped: *mut usize,
) -> c_int {
    if written.is_null() || dropped.is_null() {
        return -1;
    }
    unsafe {
        *written = 0;
        *dropped = 0;
    }
    if out.is_null() && capacity != 0 {
        return -1;
    }
    let (Some(snapshot), Some(track)) = (snapshot_ref(snapshot), read_id(track)) else {
        return -1;
    };
    let view = PianoView {
        origin_tick: Tick::new(origin_tick),
        width_ticks,
        top_pitch,
        height_pitches,
        pixels_per_tick,
        pixels_per_pitch,
    };
    let slot = if out.is_null() || capacity == 0 {
        &mut [][..]
    } else {
        unsafe { std::slice::from_raw_parts_mut(out, capacity) }
    };
    let (filled, overflow) = snapshot.write_piano(track, view, slot);
    unsafe {
        *written = filled;
        *dropped = overflow;
    }
    0
}

/// Resolves a note, including its surviving placement after an auto merge.
/// Returns 0 when found and -1 otherwise; output is unchanged on failure.
///
/// # Safety
/// Non-null UUID inputs have 16 readable bytes. Track and note are required;
/// placement is optional. `snapshot` is null or owned, and `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_piano_note(
    snapshot: *const Snapshot,
    track: *const u8,
    placement: *const u8,
    note: *const u8,
    out: *mut PianoNoteElement,
) -> c_int {
    let (Some(snapshot), Some(track), Some(note)) =
        (snapshot_ref(snapshot), read_id(track), read_id(note))
    else {
        return -1;
    };
    write(out, snapshot.piano_note(track, read_id(placement), note))
}

/// Next/previous audible note on this track, independent of the viewport.
/// Null note starts at the first (or last) note. Returns -1 at either end.
///
/// # Safety
/// Non-null UUID inputs have 16 readable bytes. Track is required; placement
/// and note are optional. `snapshot` is null or owned, and `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_piano_neighbor(
    snapshot: *const Snapshot,
    track: *const u8,
    placement: *const u8,
    note: *const u8,
    backwards: c_int,
    out: *mut PianoNoteElement,
) -> c_int {
    let (Some(snapshot), Some(track)) = (snapshot_ref(snapshot), read_id(track)) else {
        return -1;
    };
    write(
        out,
        snapshot.piano_neighbor(track, read_id(placement), read_id(note), backwards != 0),
    )
}

fn read_id(input: *const u8) -> Option<Id> {
    if input.is_null() {
        return None;
    }
    let bytes = unsafe { *(input as *const [u8; 16]) };
    Some(Id::from_raw(u128::from_be_bytes(bytes)))
}

fn write(out: *mut PianoNoteElement, note: Option<PianoNoteElement>) -> c_int {
    if out.is_null() {
        return -1;
    }
    let Some(note) = note else {
        return -1;
    };
    unsafe {
        out.write(note);
    }
    0
}
