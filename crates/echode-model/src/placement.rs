//! Placing and deleting notes once a clip is a window onto shared content.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::id::Id;
use crate::model::{
    ClipKind, Content, ModelError, Note, NoteDraft, NoteGeometry, Placement, Track, bars_touch,
    occupied_bars, overlaps_others, overlaps_span,
};
use crate::time::Tick;

// Derived from immutable placement windows during host discovery.
struct FixedBars(Vec<(i64, i64)>);

impl FixedBars {
    fn new(clips: &[Placement], bar: i64) -> Self {
        let mut ranges: Vec<_> = clips
            .iter()
            .filter_map(|clip| {
                #[cfg(test)]
                crate::model_scale_tests::fixed_wall_probe();
                (clip.kind == ClipKind::Fixed)
                    .then(|| occupied_bars(clip.start.get(), clip.length.get(), bar))
                    .flatten()
            })
            .collect();
        ranges.sort_unstable();
        // Prefix maxima allow overlap queries even if fixed windows are not ordered
        // or overlap temporarily during an edit that will later be rejected.
        let mut end = i64::MIN;
        for range in &mut ranges {
            end = end.max(range.1);
            range.1 = end;
        }
        Self(ranges)
    }

    fn blocks(&self, left: (i64, i64), right: (i64, i64)) -> bool {
        let low = left.0.min(right.0);
        let high = left.1.max(right.1);
        let end = self.0.partition_point(|range| {
            #[cfg(test)]
            crate::model_scale_tests::fixed_wall_probe();
            range.0 <= high
        });
        end.checked_sub(1)
            .is_some_and(|index| low <= self.0[index].1)
    }
}

pub(super) struct FreshIds {
    pub(super) note: Id,
    pub(super) clip: Id,
    pub(super) content: Id,
}

pub(super) fn insert(
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    track: &mut Track,
    fresh: FreshIds,
    draft: &NoteDraft,
    bar: i64,
    new_clip_ids_available: impl FnOnce() -> bool,
) -> Result<(), ModelError> {
    if let Some(index) = track
        .clips
        .iter()
        .position(|clip| contains(clip, draft.start.get()))
    {
        let owner = track.clips[index].content;
        let local = content_local(&track.clips[index], draft.start.get())?;
        push_note(contents, owner, fresh.note, local, draft)?;
        let owner = refit_auto(contents, tracks, track, owner, bar)?;
        return check_shared_notes(contents, tracks, track, owner);
    }
    let note_bars =
        occupied_bars(draft.start.get(), draft.length.get(), bar).ok_or(ModelError::InvalidNote)?;
    let mut fixed = None;
    let mut hosts = Vec::new();
    for (index, clip) in track.clips.iter().enumerate() {
        if clip.kind != ClipKind::Auto {
            continue;
        }
        let Some(clip_bars) = occupied_bars(clip.start.get(), clip.length.get(), bar) else {
            continue;
        };
        if bars_touch(clip_bars, note_bars)
            && !fixed
                .get_or_insert_with(|| FixedBars::new(&track.clips, bar))
                .blocks(clip_bars, note_bars)
        {
            hosts.push(index);
        }
    }
    if hosts.is_empty() {
        // Existing and merged placements never consume the proposed clip/content IDs.
        if !new_clip_ids_available() {
            return Err(ModelError::DuplicateId);
        }
        return create_auto(contents, track, fresh, draft, note_bars, bar);
    }
    let primary = *hosts
        .iter()
        .min_by_key(|&&index| (track.clips[index].start, track.clips[index].id))
        .unwrap();
    let owner = track.clips[primary].content;
    let local = content_local(&track.clips[primary], draft.start.get())?;
    push_note(contents, owner, fresh.note, local, draft)?;
    let owner = merge(contents, tracks, track, &hosts, bar)?;
    check_shared_notes(contents, tracks, track, owner)
}

pub(super) fn delete(
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    track: &mut Track,
    note_id: Id,
    bar: i64,
) -> Result<(), ModelError> {
    // Linked windows share one note list; search each content only once.
    let mut searched = HashSet::new();
    let owner = track
        .clips
        .iter()
        .find_map(|clip| {
            if !searched.insert(clip.content) {
                return None;
            }
            let index = crate::model::content_index(contents, clip.content)?;
            contents[index]
                .notes
                .iter()
                .any(|note| {
                    #[cfg(test)]
                    crate::model_scale_tests::note_lookup();
                    note.id == note_id
                })
                .then_some(clip.content)
        })
        .ok_or(ModelError::MissingNote)?;
    let empty = {
        let content = content_mut(contents, owner)?;
        content.notes.retain(|note| note.id != note_id);
        content.notes.is_empty()
    };
    if empty {
        remove_auto(tracks, track, owner);
        return Ok(());
    }
    refit_auto(contents, tracks, track, owner, bar).map(|_| ())
}

pub(super) fn edit(
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    track: &mut Track,
    clip_id: Id,
    note_id: Id,
    geometry: NoteGeometry,
    bar: i64,
) -> Result<(), ModelError> {
    let clip = track
        .clips
        .iter()
        .find(|clip| clip.id == clip_id)
        .ok_or(ModelError::MissingClip)?;
    let grow_window = clip.kind == ClipKind::Fixed;
    let owner = clip.content;
    let local = content_local(clip, geometry.start.get())?;
    local
        .checked_add(geometry.length.get())
        .ok_or(ModelError::InvalidNote)?;
    let content_index =
        crate::model::content_index(contents, owner).ok_or(ModelError::MissingClip)?;
    let content = &contents[content_index];
    let note_index = content
        .notes
        .iter()
        .position(|note| {
            #[cfg(test)]
            crate::model_scale_tests::note_lookup();
            note.id == note_id
        })
        .ok_or(ModelError::MissingNote)?;
    let note = &content.notes[note_index];
    let time_changed = note.start.get() != local || note.length != geometry.length;
    if time_changed || note.pitch != geometry.pitch {
        let content = Arc::make_mut(&mut contents[content_index]);
        let note = &mut content.notes[note_index];
        note.pitch = geometry.pitch;
        note.start = Tick::new(local);
        note.length = geometry.length;
    }
    // Shared unchanged contents are already sorted. Duplication can instead
    // pass an unshared content with a newly appended note of the same geometry.
    if let Some(content) = Arc::get_mut(&mut contents[content_index]) {
        crate::model::sort_notes(&mut content.notes);
    }
    // Even unchanged geometry can refit and merge adjacent auto placements.
    let owner = refit_auto(contents, tracks, track, owner, bar)?;
    if time_changed && grow_window {
        grow_edited_window(&mut track.clips, clip_id, geometry)?;
    }
    check_shared_notes(contents, tracks, track, owner)
}

/// A time edit explicitly requests that the whole note sound in this window.
/// Grow only the edited fixed placement, not its other linked windows. Auto
/// placements still refit to bars; shortening a note never trims a fixed window.
fn grow_edited_window(
    clips: &mut [Placement],
    clip_id: Id,
    geometry: NoteGeometry,
) -> Result<(), ModelError> {
    let index = clips
        .iter()
        .position(|clip| clip.id == clip_id)
        .ok_or(ModelError::MissingClip)?;
    let clip = &mut clips[index];
    if clip.kind != ClipKind::Fixed {
        return Ok(());
    }
    let old_start = clip.start.get();
    let old_end = old_start
        .checked_add(clip.length.get())
        .ok_or(ModelError::InvalidClip)?;
    let note_end = geometry
        .start
        .get()
        .checked_add(geometry.length.get())
        .ok_or(ModelError::InvalidNote)?;
    let start = old_start.min(geometry.start.get());
    let end = old_end.max(note_end);
    // Keep content time zero, and therefore every other note's sounding time.
    clip.offset = Tick::new(
        clip.offset
            .get()
            .checked_add(
                start
                    .checked_sub(old_start)
                    .ok_or(ModelError::InvalidClip)?,
            )
            .ok_or(ModelError::InvalidClip)?,
    );
    clip.start = Tick::new(start);
    clip.length = Tick::new(end.checked_sub(start).ok_or(ModelError::InvalidClip)?);
    if overlaps_others(clips, index) {
        return Err(ModelError::Overlap);
    }
    Ok(())
}

/// Every placement of `content`, including fixed links on other tracks, must
/// still land inside the time range after this content's notes change.
fn check_shared_notes(
    contents: &[Arc<Content>],
    tracks: &[Arc<Track>],
    current: &Track,
    content: Id,
) -> Result<(), ModelError> {
    let index = crate::model::content_index(contents, content).ok_or(ModelError::MissingClip)?;
    let mut bounds: Option<(i64, i64)> = None;
    for note in &contents[index].notes {
        #[cfg(test)]
        crate::model_scale_tests::shared_note_validation();
        let end = note
            .start
            .get()
            .checked_add(note.length.get())
            .ok_or(ModelError::InvalidNote)?;
        let last = end.checked_sub(1).ok_or(ModelError::InvalidNote)?;
        bounds = Some(match bounds {
            Some((first, previous_last)) => (first.min(note.start.get()), previous_last.max(last)),
            None => (note.start.get(), last),
        });
    }
    let visit = |clip: &Placement| -> Result<(), ModelError> {
        if clip.content != content {
            return Ok(());
        }
        let origin = origin(clip)?;
        // Translation preserves order, so validated local extrema cover every
        // note, including notes outside this placement's fixed window.
        if bounds.is_some_and(|(first, last)| {
            origin.checked_add(first).is_none() || origin.checked_add(last).is_none()
        }) {
            return Err(ModelError::InvalidNote);
        }
        Ok(())
    };
    for clip in &current.clips {
        visit(clip)?;
    }
    for track in tracks {
        if track.id == current.id {
            continue;
        }
        for clip in &track.clips {
            visit(clip)?;
        }
    }
    Ok(())
}

fn create_auto(
    contents: &mut Vec<Arc<Content>>,
    track: &mut Track,
    fresh: FreshIds,
    draft: &NoteDraft,
    note_bars: (i64, i64),
    bar: i64,
) -> Result<(), ModelError> {
    let start = note_bars
        .0
        .checked_mul(bar)
        .ok_or(ModelError::InvalidNote)?;
    let end = note_bars
        .1
        .checked_add(1)
        .and_then(|index| index.checked_mul(bar))
        .ok_or(ModelError::InvalidNote)?;
    let length = end.checked_sub(start).ok_or(ModelError::InvalidNote)?;
    if overlaps_span(&track.clips, start, end) {
        return Err(ModelError::Overlap);
    }
    let local = draft
        .start
        .get()
        .checked_sub(start)
        .ok_or(ModelError::InvalidNote)?;
    insert_content(
        contents,
        Content {
            id: fresh.content,
            audio: None,
            takes: Vec::new(),
            notes: vec![Note {
                id: fresh.note,
                start: Tick::new(local),
                length: draft.length,
                pitch: draft.pitch,
                velocity: crate::model::midi7_to_velocity(draft.velocity),
                release: 0,
                articulation: None,
            }],
            expressions: Vec::new(),
        },
    );
    track.clips.push(Placement::new_auto(
        fresh.clip,
        fresh.content,
        Tick::new(start),
        Tick::new(length),
    ));
    Ok(())
}

fn merge(
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    track: &mut Track,
    hosts: &[usize],
    bar: i64,
) -> Result<Id, ModelError> {
    let mut order = hosts.to_vec();
    order.sort_by(|&left, &right| {
        track.clips[left]
            .start
            .cmp(&track.clips[right].start)
            .then(track.clips[left].id.cmp(&track.clips[right].id))
    });
    let primary_index = order[0];
    let primary_id = track.clips[primary_index].id;
    let primary_content = track.clips[primary_index].content;
    let primary_origin = origin(&track.clips[primary_index])?;
    let mut absorbed = Vec::new();
    let mut retired = Vec::new();
    for index in order.iter().skip(1) {
        let placement = &track.clips[*index];
        let from_origin = origin(placement)?;
        let delta = from_origin
            .checked_sub(primary_origin)
            .ok_or(ModelError::InvalidNote)?;
        for note in notes(contents, placement.content)?.iter().cloned() {
            absorbed.push(Note {
                start: Tick::new(
                    note.start
                        .get()
                        .checked_add(delta)
                        .ok_or(ModelError::InvalidNote)?,
                ),
                ..note
            });
        }
        retired.push((placement.id, placement.content, delta));
    }
    {
        let content = content_mut(contents, primary_content)?;
        content.notes.extend(absorbed);
        crate::model::sort_notes(&mut content.notes);
    }
    let removed: HashSet<Id> = retired.iter().map(|(id, _, _)| *id).collect();
    track.clips.retain(|clip| {
        #[cfg(test)]
        crate::model_scale_tests::merge_placement_probe();
        !removed.contains(&clip.id)
    });
    let retired: HashMap<Id, i64> = retired
        .into_iter()
        .map(|(_, content, delta)| (content, delta))
        .collect();
    if !retired.is_empty() {
        retarget(tracks, track, &retired, primary_content)?;
        // All links to retired contents were retargeted, so prune those contents once.
        contents.retain(|content| {
            #[cfg(test)]
            crate::model_scale_tests::merge_content_probe();
            !retired.contains_key(&content.id)
        });
    }
    let index = track
        .clips
        .iter()
        .position(|clip| clip.id == primary_id)
        .ok_or(ModelError::MissingClip)?;
    refit_one(contents, &mut track.clips, index, bar)?;
    Ok(primary_content)
}

fn retarget(
    tracks: &mut [Arc<Track>],
    current: &mut Track,
    retired: &HashMap<Id, i64>,
    to: Id,
) -> Result<(), ModelError> {
    let edit = |clip: &mut Placement| -> Result<(), ModelError> {
        #[cfg(test)]
        crate::model_scale_tests::merge_placement_probe();
        let Some(&delta) = retired.get(&clip.content) else {
            return Ok(());
        };
        clip.offset = Tick::new(
            clip.offset
                .get()
                .checked_add(delta)
                .ok_or(ModelError::InvalidNote)?,
        );
        clip.content = to;
        Ok(())
    };
    for clip in &mut current.clips {
        edit(clip)?;
    }
    for track in tracks.iter_mut() {
        if track.id == current.id
            || !track.clips.iter().any(|clip| {
                #[cfg(test)]
                crate::model_scale_tests::merge_placement_probe();
                retired.contains_key(&clip.content)
            })
        {
            continue;
        }
        #[cfg(test)]
        crate::model_scale_tests::refit_track_copy();
        let mut owned = (**track).clone();
        for clip in &mut owned.clips {
            edit(clip)?;
        }
        *track = Arc::new(owned);
    }
    Ok(())
}

/// Returns the surviving content, which may change when auto clips merge.
fn refit_auto(
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    current: &mut Track,
    content: Id,
    bar: i64,
) -> Result<Id, ModelError> {
    if current
        .clips
        .iter()
        .any(|clip| clip.content == content && clip.kind == ClipKind::Auto)
    {
        return refit_current(contents, tracks, current, content, bar);
    }
    for slot in 0..tracks.len() {
        if tracks[slot].id == current.id {
            continue;
        }
        if tracks[slot]
            .clips
            .iter()
            .any(|clip| clip.content == content && clip.kind == ClipKind::Auto)
        {
            #[cfg(test)]
            crate::model_scale_tests::refit_track_copy();
            let mut owned = (*tracks[slot]).clone();
            let owner = refit_current(contents, tracks, &mut owned, content, bar)?;
            crate::model::sort_clips(&mut owned.clips);
            // Merge may have retargeted fixed windows on the edited track too.
            // Index once: each fixed window may hold local edits we must retain.
            if let Some(retargeted) = tracks.iter().find(|track| {
                #[cfg(test)]
                crate::model_scale_tests::refit_sync_probe();
                track.id == current.id
            }) {
                let placements: HashMap<_, _> = retargeted
                    .clips
                    .iter()
                    .map(|clip| {
                        #[cfg(test)]
                        crate::model_scale_tests::refit_sync_probe();
                        (clip.id, clip)
                    })
                    .collect();
                for clip in &mut current.clips {
                    #[cfg(test)]
                    crate::model_scale_tests::refit_sync_probe();
                    if let Some(retargeted) = placements.get(&clip.id)
                        && clip.content != retargeted.content
                    {
                        clip.content = retargeted.content;
                        clip.offset = retargeted.offset;
                    }
                }
            }
            tracks[slot] = Arc::new(owned);
            return Ok(owner);
        }
    }
    Ok(content)
}

fn refit_current(
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    current: &mut Track,
    content: Id,
    bar: i64,
) -> Result<Id, ModelError> {
    let index = current
        .clips
        .iter()
        .position(|clip| clip.content == content && clip.kind == ClipKind::Auto)
        .ok_or(ModelError::MissingClip)?;
    let span = note_bars(contents, &current.clips[index], bar)?;
    let hosts = refit_hosts(&current.clips, index, span, bar)?;
    if hosts.len() == 1 {
        refit_one(contents, &mut current.clips, index, bar)?;
        Ok(content)
    } else {
        merge(contents, tracks, current, &hosts, bar)
    }
}

/// Discover against published, start-sorted, non-overlapping windows. Their bar
/// starts and ends are monotone too, even for sub-bar moves and negative times.
fn refit_hosts(
    clips: &[Placement],
    index: usize,
    mut span: (i64, i64),
    bar: i64,
) -> Result<Vec<usize>, ModelError> {
    let mut candidates = Vec::new();
    let mut split = 0;
    for (slot, clip) in clips.iter().enumerate() {
        if slot == index || clip.kind != ClipKind::Auto {
            continue;
        }
        #[cfg(test)]
        crate::model_scale_tests::refit_host_probe();
        let range = occupied_bars(clip.start.get(), clip.length.get(), bar)
            .ok_or(ModelError::InvalidNote)?;
        split += usize::from(range.0 < span.0);
        candidates.push((slot, range));
    }
    let mut hosts = vec![index];
    let mut fixed = None;
    let mut visit = |&(slot, candidate): &(usize, (i64, i64))| {
        #[cfg(test)]
        crate::model_scale_tests::refit_host_probe();
        if !bars_touch(span, candidate) {
            return false;
        }
        if fixed
            .get_or_insert_with(|| FixedBars::new(clips, bar))
            .blocks(span, candidate)
        {
            return true;
        }
        hosts.push(slot);
        span = (span.0.min(candidate.0), span.1.max(candidate.1));
        true
    };
    // Expand left first: a containing window can extend the right frontier too.
    // Only a gap stops a side. A blocked window can share a bar with a farther
    // candidate that still touches the span without crossing the fixed wall.
    for candidate in candidates[..split].iter().rev() {
        if !visit(candidate) {
            break;
        }
    }
    for candidate in &candidates[split..] {
        if !visit(candidate) {
            break;
        }
    }
    Ok(hosts)
}

fn note_bars(
    contents: &[Arc<Content>],
    clip: &Placement,
    bar: i64,
) -> Result<(i64, i64), ModelError> {
    let found = notes(contents, clip.content)?;
    if found.is_empty() {
        return Err(ModelError::InvalidNote);
    }
    let origin = origin(clip)?;
    let mut span = (i64::MAX, i64::MIN);
    for note in found {
        let absolute = origin
            .checked_add(note.start.get())
            .ok_or(ModelError::InvalidNote)?;
        let occupied =
            occupied_bars(absolute, note.length.get(), bar).ok_or(ModelError::InvalidNote)?;
        span = (span.0.min(occupied.0), span.1.max(occupied.1));
    }
    Ok(span)
}

fn refit_one(
    contents: &[Arc<Content>],
    clips: &mut [Placement],
    index: usize,
    bar: i64,
) -> Result<(), ModelError> {
    let found = notes(contents, clips[index].content)?;
    if found.is_empty() {
        return Err(ModelError::InvalidNote);
    }
    let origin = origin(&clips[index])?;
    let mut min_bar = i64::MAX;
    let mut max_bar = i64::MIN;
    for note in found {
        let (start_bar, end_bar) = occupied_bars(note.start.get(), note.length.get(), bar)
            .ok_or(ModelError::InvalidNote)?;
        min_bar = min_bar.min(start_bar);
        max_bar = max_bar.max(end_bar);
    }
    let new_start = min_bar
        .checked_mul(bar)
        .and_then(|local| origin.checked_add(local))
        .ok_or(ModelError::InvalidNote)?;
    let new_end = max_bar
        .checked_add(1)
        .and_then(|index| index.checked_mul(bar))
        .and_then(|local| origin.checked_add(local))
        .ok_or(ModelError::InvalidNote)?;
    clips[index].offset = Tick::new(
        new_start
            .checked_sub(origin)
            .ok_or(ModelError::InvalidNote)?,
    );
    clips[index].start = Tick::new(new_start);
    clips[index].length = Tick::new(
        new_end
            .checked_sub(new_start)
            .ok_or(ModelError::InvalidNote)?,
    );
    if overlaps_others(clips, index) {
        return Err(ModelError::Overlap);
    }
    Ok(())
}

fn remove_auto(tracks: &mut [Arc<Track>], current: &mut Track, content: Id) {
    let remove = |clip: &Placement| clip.content == content && clip.kind == ClipKind::Auto;
    current.clips.retain(|clip| !remove(clip));
    for track in tracks.iter_mut() {
        if track.id == current.id || !track.clips.iter().any(remove) {
            continue;
        }
        #[cfg(test)]
        crate::model_scale_tests::delete_track_copy();
        let mut owned = (**track).clone();
        owned.clips.retain(|clip| !remove(clip));
        *track = Arc::new(owned);
    }
}

fn push_note(
    contents: &mut [Arc<Content>],
    content: Id,
    note_id: Id,
    local: i64,
    draft: &NoteDraft,
) -> Result<(), ModelError> {
    if local
        .checked_add(draft.length.get())
        .filter(|_| draft.length.get() > 0)
        .is_none()
    {
        return Err(ModelError::InvalidNote);
    }
    let notes = &mut content_mut(contents, content)?.notes;
    notes.push(Note {
        id: note_id,
        start: Tick::new(local),
        length: draft.length,
        pitch: draft.pitch,
        velocity: crate::model::midi7_to_velocity(draft.velocity),
        release: 0,
        articulation: None,
    });
    crate::model::sort_notes(notes);
    Ok(())
}

fn content_local(clip: &Placement, timeline: i64) -> Result<i64, ModelError> {
    timeline
        .checked_sub(clip.start.get())
        .and_then(|delta| delta.checked_add(clip.offset.get()))
        .ok_or(ModelError::InvalidNote)
}

fn origin(clip: &Placement) -> Result<i64, ModelError> {
    clip.start
        .get()
        .checked_sub(clip.offset.get())
        .ok_or(ModelError::InvalidNote)
}

fn notes(contents: &[Arc<Content>], id: Id) -> Result<&[Note], ModelError> {
    let index = crate::model::content_index(contents, id).ok_or(ModelError::MissingClip)?;
    Ok(&contents[index].notes)
}

fn content_mut(contents: &mut [Arc<Content>], id: Id) -> Result<&mut Content, ModelError> {
    let index = crate::model::content_index(contents, id).ok_or(ModelError::MissingClip)?;
    Ok(Arc::make_mut(&mut contents[index]))
}

fn insert_content(contents: &mut Vec<Arc<Content>>, content: Content) {
    contents.push(Arc::new(content));
    contents.sort_by_key(|content| content.id);
}

fn contains(clip: &Placement, at: i64) -> bool {
    match clip.start.get().checked_add(clip.length.get()) {
        Some(end) => at >= clip.start.get() && at < end,
        None => false,
    }
}

pub(super) fn drop_unreferenced(contents: &mut Vec<Arc<Content>>, tracks: &[Arc<Track>]) {
    let referenced = crate::model::referenced_contents(tracks);
    contents.retain(|content| referenced.contains(&content.id));
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "placement_host_tests.rs"]
mod host_tests;
