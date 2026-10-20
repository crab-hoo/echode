//! Immutable arrange projections. Collapsing rows never edits a project.
use crate::{Snapshot, snapshot_ref};
use echode_model::{Id, Project};
use std::collections::{BTreeMap, BTreeSet};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TrackTreeInfo {
    pub parent: [u8; 16],
    pub has_parent: u32,
    pub has_children: u32,
    pub depth: usize,
}

pub(crate) fn hierarchy(project: &Project) -> Vec<TrackTreeInfo> {
    let parents: BTreeSet<_> = project
        .tracks()
        .iter()
        .filter_map(|track| track.parent)
        .collect();
    let mut depths = BTreeMap::new();
    project
        .tracks()
        .iter()
        .map(|track| {
            let depth = track
                .parent
                .and_then(|parent| depths.get(&parent).copied())
                .map_or(0, |depth: usize| depth + 1);
            depths.insert(track.id, depth);
            TrackTreeInfo {
                parent: track
                    .parent
                    .map(|id| *uuid::Uuid::from_u128(id.raw()).as_bytes())
                    .unwrap_or_default(),
                has_parent: u32::from(track.parent.is_some()),
                has_children: u32::from(parents.contains(&track.id)),
                depth,
            }
        })
        .collect()
}

/// Reads hierarchy at the same displayed row as names, devices and mixers.
///
/// # Safety
/// `snapshot` is null or owned. `out` is null or writable for one value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_track_tree(
    snapshot: *const Snapshot,
    row: usize,
    out: *mut TrackTreeInfo,
) -> i32 {
    let Some(info) = snapshot_ref(snapshot).and_then(|snapshot| snapshot.track_tree(row)) else {
        return -1;
    };
    if out.is_null() {
        return -1;
    }
    unsafe { out.write(info) };
    0
}

/// Returns an owned display projection, sharing the source project and indexes.
/// Unknown/duplicate collapsed IDs are harmless. Project audio and history stay
/// unchanged. A projection can outlive its source handle and the document.
///
/// # Safety
/// `snapshot` is null or owned. `collapsed` is null for zero IDs, or addresses
/// `count * 16` readable UUID bytes. The returned handle must be freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_project_arrange(
    snapshot: *const Snapshot,
    collapsed: *const u8,
    count: usize,
) -> *mut Snapshot {
    let Some(snapshot) = snapshot_ref(snapshot) else {
        return std::ptr::null_mut();
    };
    let Some(length) = count
        .checked_mul(16)
        .filter(|length| *length <= isize::MAX as usize)
    else {
        return std::ptr::null_mut();
    };
    if length != 0 && collapsed.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = if length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(collapsed, length) }
    };
    let ids = bytes
        .chunks_exact(16)
        .map(|bytes| {
            let mut id = [0; 16];
            id.copy_from_slice(bytes);
            Id::from_raw(u128::from_be_bytes(id))
        })
        .collect::<BTreeSet<_>>();
    Box::into_raw(Box::new(snapshot.project_arrange(&ids)))
}
