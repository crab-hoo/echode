//! The C ABI's shared argument rules (ADR 0321, audit FFI-A1). These calls
//! touch only snapshots, so they also run under Miri, which reports a slice or
//! reference made from a refused argument as undefined behaviour.

use super::*;
use echode_model::{Id, MixerIds};
use std::sync::Arc;

fn snapshot() -> *mut Snapshot {
    let project = Project::new()
        .add_track_with_id(Id::from_raw(1), "keys", MixerIds::from_base(16))
        .unwrap();
    Box::into_raw(Box::new(Snapshot::from_project(Arc::new(project), None)))
}

fn element() -> VisibleElement {
    VisibleElement {
        role: u32::MAX,
        track_id: [0; 16],
        placement_id: [0; 16],
        note_id: [0; 16],
        has_placement: 0,
        has_note: 0,
        pad: [0; 2],
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }
}

/// The smallest count of `T` whose bytes exceed `isize::MAX`.
fn oversized<T>() -> usize {
    isize::MAX as usize / size_of::<T>() + 1
}

#[test]
fn contract_output_room_beyond_isize_max_is_refused_before_it_is_written() {
    let snapshot = snapshot();
    let mut elements = [element()];
    let (mut written, mut dropped) = (7usize, 7usize);
    // SAFETY: the snapshot is owned, `elements` holds one element and the
    // counts are separate locals; only the stated capacity is out of range.
    let code = unsafe {
        echode_snapshot_visible(
            snapshot,
            0,
            705_600,
            0.0,
            1.0,
            1.0,
            20.0,
            elements.as_mut_ptr(),
            oversized::<VisibleElement>(),
            &mut written,
            &mut dropped,
        )
    };
    assert_eq!((code, written, dropped), (-1, 0, 0));
    assert_eq!(elements[0].role, u32::MAX);

    let mut notes = [PianoNoteElement::default()];
    let track = *uuid::Uuid::from_u128(1).as_bytes();
    // SAFETY: as above, with 16 readable bytes for the track.
    let code = unsafe {
        piano::echode_snapshot_piano_visible(
            snapshot,
            track.as_ptr(),
            0,
            705_600,
            127.0,
            128.0,
            1.0,
            1.0,
            notes.as_mut_ptr(),
            oversized::<PianoNoteElement>(),
            &mut written,
            &mut dropped,
        )
    };
    assert_eq!((code, written, dropped), (-1, 0, 0));

    let (mut min, mut max) = ([9.0f32], [9.0f32]);
    let placement = [0u8; 16];
    // SAFETY: as above, with one float in each of two separate arrays.
    let columns = unsafe {
        echode_snapshot_waveform(
            snapshot,
            placement.as_ptr(),
            0,
            705_600,
            min.as_mut_ptr(),
            max.as_mut_ptr(),
            oversized::<f32>(),
        )
    };
    assert_eq!((columns, min, max), (0, [9.0], [9.0]));
    // SAFETY: the snapshot is owned and freed once.
    unsafe { echode_snapshot_free(snapshot) };
}

#[test]
fn contract_null_memory_with_a_length_is_refused() {
    let snapshot = snapshot();
    let (mut written, mut dropped) = (7usize, 7usize);
    // SAFETY: the snapshot is owned and the counts are separate locals; the
    // null `out` with room for one element is the refused argument.
    let code = unsafe {
        echode_snapshot_visible(
            snapshot,
            0,
            705_600,
            0.0,
            1.0,
            1.0,
            20.0,
            std::ptr::null_mut(),
            1,
            &mut written,
            &mut dropped,
        )
    };
    assert_eq!((code, written, dropped), (-1, 0, 0));
    let mut min = [9.0f32];
    let placement = [0u8; 16];
    // SAFETY: as above, with a null `maxs` for one float.
    let columns = unsafe {
        echode_snapshot_waveform(
            snapshot,
            placement.as_ptr(),
            0,
            705_600,
            min.as_mut_ptr(),
            std::ptr::null_mut(),
            1,
        )
    };
    assert_eq!((columns, min), (0, [9.0]));
    // SAFETY: the snapshot is owned; one null UUID is the refused input.
    let projection = unsafe { echode_snapshot_project_arrange(snapshot, std::ptr::null(), 1) };
    assert!(projection.is_null());
    // SAFETY: the snapshot is owned and freed once.
    unsafe { echode_snapshot_free(snapshot) };
}

#[test]
fn contract_misaligned_out_parameters_are_refused_and_left_unchanged() {
    let snapshot = snapshot();
    // Five words leave room for a 32-byte `TrackTreeInfo` that starts one byte in.
    let mut words = [u64::MAX; 5];
    let misaligned = words.as_mut_ptr().cast::<u8>().wrapping_add(1);
    // SAFETY: the snapshot is owned. The misaligned pointers address enough
    // writable bytes inside `words`, and alignment is the refused property.
    let (name, flicks, tree) = unsafe {
        (
            echode_snapshot_track_name(snapshot, 0, misaligned.cast::<usize>()),
            echode_snapshot_flicks(snapshot, 0, misaligned.cast::<i64>()),
            echode_snapshot_track_tree(snapshot, 0, misaligned.cast::<TrackTreeInfo>()),
        )
    };
    assert!(name.is_null());
    assert_eq!((flicks, tree), (ECHODE_ERR, -1));
    assert_eq!(words, [u64::MAX; 5]);

    let mut written = 7usize;
    let mut dropped = [u64::MAX; 2];
    // SAFETY: the snapshot is owned and `written` is a local; `dropped` is the
    // misaligned count inside a local array.
    let code = unsafe {
        echode_snapshot_visible(
            snapshot,
            0,
            705_600,
            0.0,
            1.0,
            1.0,
            20.0,
            std::ptr::null_mut(),
            0,
            &mut written,
            dropped
                .as_mut_ptr()
                .cast::<u8>()
                .wrapping_add(1)
                .cast::<usize>(),
        )
    };
    assert_eq!((code, written, dropped), (-1, 7, [u64::MAX; 2]));

    let mut room = [element(); 2];
    let shifted = room
        .as_mut_ptr()
        .cast::<u8>()
        .wrapping_add(1)
        .cast::<VisibleElement>();
    let mut dropped = 7usize;
    // SAFETY: the snapshot is owned and the counts are locals; the element
    // room starts one byte into `room`, which still holds one whole element.
    let code = unsafe {
        echode_snapshot_visible(
            snapshot,
            0,
            705_600,
            0.0,
            1.0,
            1.0,
            20.0,
            shifted,
            1,
            &mut written,
            &mut dropped,
        )
    };
    assert_eq!((code, written, dropped), (-1, 0, 0));
    assert!(room.iter().all(|element| element.role == u32::MAX));
    // SAFETY: the snapshot is owned and freed once.
    unsafe { echode_snapshot_free(snapshot) };
}

#[test]
fn contract_uuids_are_read_at_any_alignment() {
    let snapshot = snapshot();
    // A UUID one byte into a buffer: no UUID input needs alignment.
    let mut bytes = [0u8; 17];
    bytes[1..].copy_from_slice(uuid::Uuid::from_u128(1).as_bytes());
    // SAFETY: the snapshot is owned and one UUID's 16 bytes are readable.
    let projection = unsafe { echode_snapshot_project_arrange(snapshot, bytes[1..].as_ptr(), 1) };
    assert!(!projection.is_null());
    // SAFETY: both handles are owned and each is freed once.
    unsafe {
        echode_snapshot_free(projection);
        echode_snapshot_free(snapshot);
    }
}
