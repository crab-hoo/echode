//! Immutable arrange projections. Collapsing rows never edits a project.
use crate::{Snapshot, abi};
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
    // SAFETY: the caller passes null or an owned handle.
    let Some(info) = unsafe { abi::handle(snapshot) }.and_then(|snapshot| snapshot.track_tree(row))
    else {
        return -1;
    };
    // SAFETY: the caller passes null or one writable value.
    if unsafe { abi::put(out, info) } {
        0
    } else {
        -1
    }
}

/// Returns an owned display projection, sharing the source project and indexes.
/// Unknown/duplicate collapsed IDs are harmless. Project audio and history stay
/// unchanged. A projection can outlive its source handle and the document.
///
/// # Safety
/// `snapshot` is null or owned. `collapsed` is null or addresses
/// `count * 16` readable UUID bytes. The returned handle must be freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_project_arrange(
    snapshot: *const Snapshot,
    collapsed: *const u8,
    count: usize,
) -> *mut Snapshot {
    // SAFETY: the caller passes null or an owned handle, and `count` UUIDs
    // of 16 readable bytes each at a non-null `collapsed`.
    let (Some(snapshot), Some(ids)) = (unsafe {
        (
            abi::handle(snapshot),
            abi::input(collapsed.cast::<[u8; 16]>(), count),
        )
    }) else {
        return std::ptr::null_mut();
    };
    let ids = ids
        .iter()
        .map(|id| Id::from_raw(u128::from_be_bytes(*id)))
        .collect::<BTreeSet<_>>();
    Box::into_raw(Box::new(snapshot.project_arrange(&ids)))
}
