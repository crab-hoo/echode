//! Placing and deleting notes once a clip is a window onto shared content.

use std::sync::Arc;

use crate::id::Id;
use crate::model::{
    ClipKind, Content, ModelError, Note, NoteDraft, Placement, Track, bars_touch, fixed_blocks,
    occupied_bars, overlaps_others, overlaps_span,
};
use crate::time::Tick;

pub(super) struct FreshIds {
    pub(super) note: Id,
    pub(super) clip: Id,
    pub(super) content: Id,
    pub(super) clip_free: bool,
    pub(super) content_free: bool,
}

pub(super) fn insert(
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    track: &mut Track,
    fresh: FreshIds,
    draft: &NoteDraft,
    bar: i64,
) -> Result<(), ModelError> {
    if let Some(index) = track
        .clips
        .iter()
        .position(|clip| contains(clip, draft.start.get()))
    {
        let owner = track.clips[index].content;
        let local = content_local(&track.clips[index], draft.start.get())?;
        push_note(contents, owner, fresh.note, local, draft)?;
        return refit_auto(contents, tracks, track, owner, bar);
    }
    let note_bars =
        occupied_bars(draft.start.get(), draft.length.get(), bar).ok_or(ModelError::InvalidNote)?;
    let mut hosts = Vec::new();
    for (index, clip) in track.clips.iter().enumerate() {
        if clip.kind != ClipKind::Auto {
            continue;
        }
        let Some(clip_bars) = occupied_bars(clip.start.get(), clip.length.get(), bar) else {
            continue;
        };
        if bars_touch(clip_bars, note_bars)
            && !fixed_blocks(&track.clips, clip_bars, note_bars, bar)
        {
            hosts.push(index);
        }
    }
    if hosts.is_empty() {
        return create_auto(contents, track, fresh, draft, note_bars, bar);
    }
    merge(contents, tracks, track, &hosts, fresh.note, draft, bar)
}

pub(super) fn delete(
    contents: &mut [Arc<Content>],
    tracks: &mut [Arc<Track>],
    track: &mut Track,
    note_id: Id,
    bar: i64,
) -> Result<(), ModelError> {
    let owner = track
        .clips
        .iter()
        .find_map(|clip| {
            notes(contents, clip.content)
                .ok()
                .filter(|found| found.iter().any(|note| note.id == note_id))
                .map(|_| clip.content)
        })
        .ok_or(ModelError::MissingNote)?;
    {
        let content = content_mut(contents, owner)?;
        let before = content.notes.len();
        content.notes.retain(|note| note.id != note_id);
        if content.notes.len() == before {
            return Err(ModelError::MissingNote);
        }
    }
    if notes(contents, owner)?.is_empty() {
        remove_auto(tracks, track, owner);
        return Ok(());
    }
    refit_auto(contents, tracks, track, owner, bar)
}

fn create_auto(
    contents: &mut Vec<Arc<Content>>,
    track: &mut Track,
    fresh: FreshIds,
    draft: &NoteDraft,
    note_bars: (i64, i64),
    bar: i64,
) -> Result<(), ModelError> {
    if !fresh.clip_free || !fresh.content_free {
        return Err(ModelError::DuplicateId);
    }
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
    note_id: Id,
    draft: &NoteDraft,
    bar: i64,
) -> Result<(), ModelError> {
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
        for note in notes(contents, placement.content)? {
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
        retired.push((placement.id, placement.content, from_origin));
    }
    let local = draft
        .start
        .get()
        .checked_sub(primary_origin)
        .ok_or(ModelError::InvalidNote)?;
    absorbed.push(Note {
        id: note_id,
        start: Tick::new(local),
        length: draft.length,
        pitch: draft.pitch,
        velocity: crate::model::midi7_to_velocity(draft.velocity),
        release: 0,
        articulation: None,
    });
    {
        let content = content_mut(contents, primary_content)?;
        content.notes.extend(absorbed);
        crate::model::sort_notes(&mut content.notes);
    }
    let removed: Vec<Id> = retired.iter().map(|(id, _, _)| *id).collect();
    track.clips.retain(|clip| !removed.contains(&clip.id));
    for (_, content, _origin) in &retired {
        retarget(tracks, track, *content, primary_content, primary_origin)?;
        if !referenced(tracks, track, *content) {
            contents.retain(|item| item.id != *content);
        }
    }
    let index = track
        .clips
        .iter()
        .position(|clip| clip.id == primary_id)
        .ok_or(ModelError::MissingClip)?;
    refit_one(contents, &mut track.clips, index, bar)?;
    Ok(())
}

fn retarget(
    tracks: &mut [Arc<Track>],
    current: &mut Track,
    from: Id,
    to: Id,
    origin_to: i64,
) -> Result<(), ModelError> {
    let edit = |clip: &mut Placement| -> Result<(), ModelError> {
        if clip.content != from {
            return Ok(());
        }
        clip.offset = Tick::new(
            clip.start
                .get()
                .checked_sub(origin_to)
                .ok_or(ModelError::InvalidNote)?,
        );
        clip.content = to;
        Ok(())
    };
    for clip in &mut current.clips {
        edit(clip)?;
    }
    for track in tracks.iter_mut() {
        if track.id == current.id {
            continue;
        }
        let mut owned = (**track).clone();
        let mut changed = false;
        for clip in &mut owned.clips {
            if clip.content == from {
                edit(clip)?;
                changed = true;
            }
        }
        if changed {
            *track = Arc::new(owned);
        }
    }
    Ok(())
}

fn refit_auto(
    contents: &[Arc<Content>],
    tracks: &mut [Arc<Track>],
    current: &mut Track,
    content: Id,
    bar: i64,
) -> Result<(), ModelError> {
    if let Some(index) = current
        .clips
        .iter()
        .position(|clip| clip.content == content && clip.kind == ClipKind::Auto)
    {
        return refit_one(contents, &mut current.clips, index, bar);
    }
    for track in tracks.iter_mut() {
        if track.id == current.id {
            continue;
        }
        let mut owned = (**track).clone();
        if let Some(index) = owned
            .clips
            .iter()
            .position(|clip| clip.content == content && clip.kind == ClipKind::Auto)
        {
            refit_one(contents, &mut owned.clips, index, bar)?;
            *track = Arc::new(owned);
            return Ok(());
        }
    }
    Ok(())
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
    for note in &found {
        let absolute = origin
            .checked_add(note.start.get())
            .ok_or(ModelError::InvalidNote)?;
        let (start_bar, end_bar) =
            occupied_bars(absolute, note.length.get(), bar).ok_or(ModelError::InvalidNote)?;
        min_bar = min_bar.min(start_bar);
        max_bar = max_bar.max(end_bar);
    }
    let new_start = min_bar.checked_mul(bar).ok_or(ModelError::InvalidNote)?;
    let new_end = max_bar
        .checked_add(1)
        .and_then(|index| index.checked_mul(bar))
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
    current
        .clips
        .retain(|clip| !(clip.content == content && clip.kind == ClipKind::Auto));
    for track in tracks.iter_mut() {
        if track.id == current.id {
            continue;
        }
        let mut owned = (**track).clone();
        let before = owned.clips.len();
        owned
            .clips
            .retain(|clip| !(clip.content == content && clip.kind == ClipKind::Auto));
        if owned.clips.len() != before {
            *track = Arc::new(owned);
        }
    }
}

fn push_note(
    contents: &mut [Arc<Content>],
    content: Id,
    note_id: Id,
    local: i64,
    draft: &NoteDraft,
) -> Result<(), ModelError> {
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

fn notes(contents: &[Arc<Content>], id: Id) -> Result<Vec<Note>, ModelError> {
    contents
        .iter()
        .find(|content| content.id == id)
        .map(|content| content.notes.clone())
        .ok_or(ModelError::MissingClip)
}

fn content_mut(contents: &mut [Arc<Content>], id: Id) -> Result<&mut Content, ModelError> {
    let content = contents
        .iter_mut()
        .find(|content| content.id == id)
        .ok_or(ModelError::MissingClip)?;
    Ok(Arc::make_mut(content))
}

fn insert_content(contents: &mut Vec<Arc<Content>>, content: Content) {
    contents.push(Arc::new(content));
    contents.sort_by_key(|content| content.id);
}

fn referenced(tracks: &[Arc<Track>], current: &Track, content: Id) -> bool {
    current.clips.iter().any(|clip| clip.content == content)
        || tracks.iter().any(|track| {
            track.id != current.id && track.clips.iter().any(|clip| clip.content == content)
        })
}

fn contains(clip: &Placement, at: i64) -> bool {
    match clip.start.get().checked_add(clip.length.get()) {
        Some(end) => at >= clip.start.get() && at < end,
        None => false,
    }
}

pub(super) fn drop_unreferenced(contents: &mut Vec<Arc<Content>>, tracks: &[Arc<Track>]) {
    contents.retain(|content| {
        tracks
            .iter()
            .any(|track| track.clips.iter().any(|clip| clip.content == content.id))
    });
}
