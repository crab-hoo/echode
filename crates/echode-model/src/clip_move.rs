//! Moving and copying a placement onto a track in replace mode (ADR 0154).

use std::collections::HashSet;
use std::sync::Arc;

use crate::duplicate::derived_id;
use crate::id::Id;
use crate::model::{
    ClipKind, Content, DEFAULT_FADE_FLICKS, ModelError, Placement, Project, Track, sort_clips,
    sort_notes,
};
use crate::time::Tick;

/// Domain separation for the copy made by [`Project::duplicate_clip`].
const COPY_CONTEXT: &str = "echode 2026-10-25 clip.duplicate object id";
/// Domain separation for the right piece of a placement that a landing window splits.
const SPLIT_CONTEXT: &str = "echode 2026-10-25 clip replace split piece id";

impl Project {
    /// Moves a placement to `track` at `start`. It keeps its id, content,
    /// window length, offset, gain, and fades, so sounding times shift with it.
    ///
    /// The landing window replaces what it covers on the target track (see
    /// [`land`]). An auto placement stays auto only when it moves by whole
    /// bars, because its window must keep matching its notes' bars. Any other
    /// shift makes it fixed.
    pub fn move_clip(
        &self,
        clip: Id,
        track: Id,
        start: Tick,
        seed: Id,
    ) -> Result<Self, ModelError> {
        let (source, placement) = self.locate_clip(clip)?;
        let landed = self.landing(placement, start)?;
        let target = self.track_index(track)?;
        let mut used = self.used_ids();
        let mut tracks = self.tracks().to_vec();
        let mut origin = Track::clone(&tracks[source]);
        origin.clips.retain(|placement| placement.id != clip);
        tracks[source] = Arc::new(origin);
        land(&mut tracks, target, landed, seed, &mut used)?;
        let mut contents = self.contents().to_vec();
        crate::placement::drop_unreferenced(&mut contents, &tracks);
        Ok(self.with_parts(contents, tracks))
    }

    /// Copies a placement to `track` at `start` with its own content.
    ///
    /// The copy keeps the window, gain, and fades. Its placement, content, and
    /// notes get ids derived from `seed`, so a note edit in the copy never
    /// reaches the original. Kind and replacement follow [`Self::move_clip`].
    pub fn duplicate_clip(
        &self,
        clip: Id,
        track: Id,
        start: Tick,
        seed: Id,
    ) -> Result<Self, ModelError> {
        let (_, placement) = self.locate_clip(clip)?;
        let mut landed = self.landing(placement, start)?;
        let target = self.track_index(track)?;
        let original = self
            .content(placement.content)
            .ok_or(ModelError::MissingClip)?;
        let mut used = self.used_ids();
        let mut content = Content::clone(original);
        {
            let mut fresh = |original: Id| {
                let id = derived_id(COPY_CONTEXT, seed, original);
                if used.insert(id) {
                    Ok(id)
                } else {
                    Err(ModelError::DuplicateId)
                }
            };
            landed.id = fresh(placement.id)?;
            content.id = fresh(original.id)?;
            for note in &mut content.notes {
                note.id = fresh(note.id)?;
            }
        }
        // Notes tie-break on id, so new ids can change their order.
        sort_notes(&mut content.notes);
        landed.content = content.id;
        let mut contents = self.contents().to_vec();
        contents.push(Arc::new(content));
        contents.sort_by_key(|content| content.id);
        let mut tracks = self.tracks().to_vec();
        land(&mut tracks, target, landed, seed, &mut used)?;
        crate::placement::drop_unreferenced(&mut contents, &tracks);
        Ok(self.with_parts(contents, tracks))
    }

    fn locate_clip(&self, clip: Id) -> Result<(usize, &Placement), ModelError> {
        self.tracks()
            .iter()
            .enumerate()
            .find_map(|(index, track)| {
                track
                    .clips
                    .iter()
                    .find(|placement| placement.id == clip)
                    .map(|placement| (index, placement))
            })
            .ok_or(ModelError::MissingClip)
    }

    /// `placement` at `start`, or why it cannot sit there.
    fn landing(&self, placement: &Placement, start: Tick) -> Result<Placement, ModelError> {
        let bar = self
            .meter()
            .bar_ticks()
            .ok_or(ModelError::InvalidMeter)?
            .get();
        if start.get() < 0 || start.get().checked_add(placement.length.get()).is_none() {
            return Err(ModelError::InvalidClip);
        }
        let origin = start
            .get()
            .checked_sub(placement.offset.get())
            .ok_or(ModelError::InvalidClip)?;
        self.notes_fit(placement.content, origin)?;
        let shift = start
            .get()
            .checked_sub(placement.start.get())
            .ok_or(ModelError::InvalidClip)?;
        let mut landed = placement.clone();
        landed.start = start;
        if shift.rem_euclid(bar) != 0 {
            landed.kind = ClipKind::Fixed;
        }
        Ok(landed)
    }
}

/// Puts `landed` on `tracks[target]` and clears its window there.
///
/// A placement inside the window is removed. One that crosses an edge keeps
/// the part outside. One that holds the whole window is split, and the right
/// piece gets an id derived from `seed`. Kept parts stay on the same content
/// and origin, become fixed, and take the default short fade on each new cut.
/// Uncut edges keep their fades.
fn land(
    tracks: &mut [Arc<Track>],
    target: usize,
    landed: Placement,
    seed: Id,
    used: &mut HashSet<Id>,
) -> Result<(), ModelError> {
    let start = landed.start.get();
    let end = start
        .checked_add(landed.length.get())
        .ok_or(ModelError::InvalidClip)?;
    let mut track = Track::clone(&tracks[target]);
    let mut kept = Vec::with_capacity(track.clips.len() + 2);
    for clip in track.clips.drain(..) {
        let clip_start = clip.start.get();
        let clip_end = clip_start
            .checked_add(clip.length.get())
            .ok_or(ModelError::InvalidClip)?;
        if clip_end <= start || end <= clip_start {
            kept.push(clip);
            continue;
        }
        let split = clip_start < start && end < clip_end;
        if clip_start < start {
            let mut left = clip.clone();
            left.length = Tick::new(start - clip_start);
            left.kind = ClipKind::Fixed;
            left.fade_out = DEFAULT_FADE_FLICKS;
            kept.push(left);
        }
        if end < clip_end {
            let mut right = clip;
            if split {
                let id = derived_id(SPLIT_CONTEXT, seed, right.id);
                if !used.insert(id) {
                    return Err(ModelError::DuplicateId);
                }
                right.id = id;
            }
            right.offset = Tick::new(
                right
                    .offset
                    .get()
                    .checked_add(end - clip_start)
                    .ok_or(ModelError::InvalidClip)?,
            );
            right.start = Tick::new(end);
            right.length = Tick::new(clip_end - end);
            right.kind = ClipKind::Fixed;
            right.fade_in = DEFAULT_FADE_FLICKS;
            kept.push(right);
        }
    }
    kept.push(landed);
    sort_clips(&mut kept);
    track.clips = kept;
    tracks[target] = Arc::new(track);
    Ok(())
}

#[cfg(test)]
#[path = "clip_move_tests.rs"]
mod tests;
