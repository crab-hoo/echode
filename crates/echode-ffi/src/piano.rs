use crate::{PianoNoteElement, Snapshot, abi};
use echode_draw::PianoView;
use echode_model::{Id, Tick};
use std::os::raw::c_int;

/// Writes piano notes intersecting time and pitch. Null output with capacity
/// zero counts notes in `dropped`. Invalid handles and a refused `out` (see
/// `abi`) return -1 and zero counts.
///
/// # Safety
/// Non-null UUID inputs have 16 readable bytes. `snapshot` is null or owned.
/// `out` is null or has room for `capacity` elements. Count pointers are null
/// or writable and do not alias.
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
    // SAFETY: the caller passes writable, unaliased counts.
    let (Some(mut written), Some(mut dropped)) =
        (unsafe { (abi::place(written), abi::place(dropped)) })
    else {
        return -1;
    };
    written.set(0);
    dropped.set(0);
    // SAFETY: the caller passes null or an owned handle, null or 16 readable
    // bytes at `track`, and room for `capacity` elements at a non-null `out`.
    let (Some(snapshot), Some(track), Some(slot)) = (unsafe {
        (
            abi::handle(snapshot),
            abi::uuid(track),
            abi::output(out, capacity),
        )
    }) else {
        return -1;
    };
    let track = Id::from_raw(track);
    let view = PianoView {
        origin_tick: Tick::new(origin_tick),
        width_ticks,
        top_pitch,
        height_pitches,
        pixels_per_tick,
        pixels_per_pitch,
    };
    let (filled, overflow) = snapshot.write_piano(track, view, slot);
    written.set(filled);
    dropped.set(overflow);
    0
}

/// Resolves a note, including its surviving placement after an auto merge.
/// Returns 0 when found and -1 otherwise; output is unchanged on failure.
///
/// # Safety
/// Non-null UUID inputs have 16 readable bytes. Track and note are required;
/// placement is optional. `snapshot` is null or owned, and `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_piano_note(
    snapshot: *const Snapshot,
    track: *const u8,
    placement: *const u8,
    note: *const u8,
    out: *mut PianoNoteElement,
) -> c_int {
    // SAFETY: the caller passes null or an owned handle, and null or 16
    // readable bytes at each UUID.
    let (Some(snapshot), Some(track), placement, Some(note)) = (unsafe {
        (
            abi::handle(snapshot),
            read_id(track),
            read_id(placement),
            read_id(note),
        )
    }) else {
        return -1;
    };
    // SAFETY: the caller passes null or one writable element.
    unsafe { write(out, snapshot.piano_note(track, placement, note)) }
}

/// Next/previous audible note on this track, independent of the viewport.
/// Null note starts at the first (or last) note. Returns -1 at either end.
///
/// # Safety
/// Non-null UUID inputs have 16 readable bytes. Track is required; placement
/// and note are optional. `snapshot` is null or owned, and `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_piano_neighbor(
    snapshot: *const Snapshot,
    track: *const u8,
    placement: *const u8,
    note: *const u8,
    backwards: c_int,
    out: *mut PianoNoteElement,
) -> c_int {
    // SAFETY: the caller passes null or an owned handle, and null or 16
    // readable bytes at each UUID.
    let (Some(snapshot), Some(track), placement, note) = (unsafe {
        (
            abi::handle(snapshot),
            read_id(track),
            read_id(placement),
            read_id(note),
        )
    }) else {
        return -1;
    };
    // SAFETY: the caller passes null or one writable element.
    unsafe {
        write(
            out,
            snapshot.piano_neighbor(track, placement, note, backwards != 0),
        )
    }
}

/// # Safety
/// A non-null `input` addresses 16 readable bytes.
unsafe fn read_id(input: *const u8) -> Option<Id> {
    // SAFETY: the caller's contract is `abi::uuid`'s.
    unsafe { abi::uuid(input) }.map(Id::from_raw)
}

/// Writes a found note. -1, writing nothing, when the note or `out` is missing.
///
/// # Safety
/// `out` is null or one writable element.
unsafe fn write(out: *mut PianoNoteElement, note: Option<PianoNoteElement>) -> c_int {
    let Some(note) = note else {
        return -1;
    };
    // SAFETY: the caller's contract is `abi::put`'s.
    if unsafe { abi::put(out, note) } {
        0
    } else {
        -1
    }
}
