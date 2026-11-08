//! Heap estimate of what one project version keeps beyond another (FCA-19).
//!
//! History holds whole versions, and an edit replaces only the tracks and
//! contents it changes. A version therefore costs what it does not share with
//! the version next to it. Shared objects are found with `Arc::ptr_eq`.

use std::collections::HashMap;
use std::mem::{size_of, size_of_val};
use std::sync::Arc;

use serde::Serialize;

use crate::{
    Change, ChangeSet, Content, Device, Expression, ExpressionPoint, Field, Id, Note, Parameter,
    Placement, Project, TempoPoint, Track,
};

/// The strong and weak counts in front of every `Arc` allocation.
const ARC_COUNTS: usize = 2 * size_of::<usize>();

/// Objects one project version keeps that another version does not share.
///
/// `bytes` adds `size_of` for each kept value and the capacity of the vectors
/// and strings it owns; a version's own lists count by length. Allocator rounding and headers are not counted, so the
/// process holds somewhat more than this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Footprint {
    pub tracks: u64,
    pub contents: u64,
    pub notes: u64,
    pub bytes: u64,
}

impl std::ops::AddAssign for Footprint {
    fn add_assign(&mut self, other: Self) {
        self.tracks = self.tracks.saturating_add(other.tracks);
        self.contents = self.contents.saturating_add(other.contents);
        self.notes = self.notes.saturating_add(other.notes);
        self.bytes = self.bytes.saturating_add(other.bytes);
    }
}

impl Footprint {
    fn track(&mut self, track: &Track) {
        self.tracks = self.tracks.saturating_add(1);
        self.add_bytes(track_bytes(track));
    }

    fn content(&mut self, content: &Content) {
        self.contents = self.contents.saturating_add(1);
        self.notes = self.notes.saturating_add(content.notes.len() as u64);
        self.add_bytes(content_bytes(content));
    }

    fn add_bytes(&mut self, bytes: usize) {
        self.bytes = self.bytes.saturating_add(bytes as u64);
    }
}

impl Project {
    /// Everything this version holds.
    pub fn footprint(&self) -> Footprint {
        let mut footprint = self.own_lists(false);
        for track in self.tracks() {
            footprint.track(track);
        }
        for content in self.contents() {
            footprint.content(content);
        }
        footprint
    }

    /// What this version holds that `neighbor` does not share.
    ///
    /// A track or content counts unless `neighbor` holds the same `Arc` under
    /// its id. The version's own lists always count, since every version owns
    /// them. Time is O(tracks + contents) of both versions.
    pub fn footprint_beyond(&self, neighbor: &Project) -> Footprint {
        let mut footprint = self.own_lists(self.tempo().shares_points(neighbor.tempo()));
        let mut by_id: Option<HashMap<Id, &Arc<Track>>> = None;
        for (index, track) in self.tracks().iter().enumerate() {
            // Most edits keep the arrange order, so the same slot usually matches.
            if neighbor
                .tracks()
                .get(index)
                .is_some_and(|same| Arc::ptr_eq(track, same))
            {
                continue;
            }
            let tracks = by_id.get_or_insert_with(|| {
                neighbor
                    .tracks()
                    .iter()
                    .map(|item| (item.id, item))
                    .collect()
            });
            if !tracks
                .get(&track.id)
                .is_some_and(|same| Arc::ptr_eq(track, same))
            {
                footprint.track(track);
            }
        }
        // Contents are sorted by id in every valid project.
        for content in self.contents() {
            let shared = neighbor
                .contents()
                .binary_search_by_key(&content.id, |item| item.id)
                .ok()
                .and_then(|index| neighbor.contents().get(index))
                .is_some_and(|same| Arc::ptr_eq(content, same));
            if !shared {
                footprint.content(content);
            }
        }
        footprint
    }

    fn own_lists(&self, tempo_shared: bool) -> Footprint {
        let mut bytes =
            size_of::<Project>() + size_of_val(self.tracks()) + size_of_val(self.contents());
        if !tempo_shared {
            // The point array and its index, which holds one segment per point.
            let points = self.tempo().points().len();
            bytes += 2 * ARC_COUNTS
                + points * (size_of::<TempoPoint>() + size_of::<echode_time::TempoSegment>());
        }
        let mut footprint = Footprint::default();
        footprint.add_bytes(bytes);
        footprint
    }
}

impl ChangeSet {
    /// Estimated bytes of the change records. Text and ID lists inside the
    /// values are not counted.
    pub fn estimated_bytes(&self) -> usize {
        size_of::<ChangeSet>()
            + self
                .changes()
                .iter()
                .map(|change| {
                    size_of::<Change>()
                        + match change {
                            Change::Create { fields, .. } | Change::Delete { fields, .. } => {
                                fields.len() * size_of::<Field>()
                            }
                            Change::Update { .. } => 0,
                        }
                })
                .sum::<usize>()
    }
}

fn track_bytes(track: &Track) -> usize {
    ARC_COUNTS
        + size_of::<Track>()
        + track.name.capacity()
        + track.order.capacity()
        + track.clips.capacity() * size_of::<Placement>()
        + track.devices.capacity() * size_of::<Device>()
        + track.devices.iter().map(device_heap).sum::<usize>()
}

fn device_heap(device: &Device) -> usize {
    device.plugin.as_ref().map_or(0, String::capacity)
        + device.state.as_ref().map_or(0, String::capacity)
        + device.pinned.capacity() * size_of::<u32>()
        + device.parameters.capacity() * size_of::<Parameter>()
}

fn content_bytes(content: &Content) -> usize {
    ARC_COUNTS
        + size_of::<Content>()
        + content.notes.capacity() * size_of::<Note>()
        + content.expressions.capacity() * size_of::<Expression>()
        + content
            .expressions
            .iter()
            .map(|curve| curve.points.capacity() * size_of::<ExpressionPoint>())
            .sum::<usize>()
        + content.takes.capacity() * size_of::<Id>()
        + content
            .audio
            .as_ref()
            .map_or(0, |audio| audio.hash.capacity())
}

#[cfg(test)]
#[path = "footprint_tests.rs"]
mod tests;
