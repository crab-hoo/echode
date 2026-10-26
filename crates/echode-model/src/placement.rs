//! Placing and deleting notes once a clip is a window onto shared content.

use std::sync::Arc;

use crate::id::Id;
use crate::model::{
    ClipKind, Content, ModelError, Note, NoteDraft, NoteGeometry, Placement, Track, bars_touch,
    fixed_blocks, occupied_bars, overlaps_others, overlaps_span,
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
        refit_auto(contents, tracks, track, owner, bar)?;
        return check_shared_notes(
            contents,
            tracks,
            track,
            content_of_note(contents, fresh.note).unwrap_or(owner),
        );
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
    let primary = *hosts
        .iter()
        .min_by_key(|&&index| (track.clips[index].start, track.clips[index].id))
        .unwrap();
    let owner = track.clips[primary].content;
    let local = content_local(&track.clips[primary], draft.start.get())?;
    push_note(contents, owner, fresh.note, local, draft)?;
    merge(contents, tracks, track, &hosts, bar)?;
    check_shared_notes(
        contents,
        tracks,
        track,
        content_of_note(contents, fresh.note).unwrap_or(owner),
    )
}

pub(super) fn delete(
    contents: &mut Vec<Arc<Content>>,
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
    let content = content_mut(contents, owner)?;
    let note = content
        .notes
        .iter_mut()
        .find(|note| note.id == note_id)
        .ok_or(ModelError::MissingNote)?;
    let time_changed = note.start.get() != local || note.length != geometry.length;
    note.pitch = geometry.pitch;
    note.start = Tick::new(local);
    note.length = geometry.length;
    crate::model::sort_notes(&mut content.notes);
    refit_auto(contents, tracks, track, owner, bar)?;
    if time_changed && grow_window {
        grow_edited_window(&mut track.clips, clip_id, geometry)?;
    }
    check_shared_notes(
        contents,
        tracks,
        track,
        content_of_note(contents, note_id).unwrap_or(owner),
    )
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

fn content_of_note(contents: &[Arc<Content>], note: Id) -> Option<Id> {
    contents.iter().find_map(|content| {
        content
            .notes
            .iter()
            .any(|item| item.id == note)
            .then_some(content.id)
    })
}

/// Every placement of `content`, including fixed links on other tracks, must
/// still land inside the time range after this content's notes change.
fn check_shared_notes(
    contents: &[Arc<Content>],
    tracks: &[Arc<Track>],
    current: &Track,
    content: Id,
) -> Result<(), ModelError> {
    let found = notes(contents, content)?;
    let visit = |clip: &Placement| -> Result<(), ModelError> {
        if clip.content != content {
            return Ok(());
        }
        let origin = origin(clip)?;
        for note in &found {
            let absolute = origin
                .checked_add(note.start.get())
                .ok_or(ModelError::InvalidNote)?;
            let span = note
                .length
                .get()
                .checked_sub(1)
                .ok_or(ModelError::InvalidNote)?;
            absolute.checked_add(span).ok_or(ModelError::InvalidNote)?;
            note.start
                .get()
                .checked_add(note.length.get())
                .ok_or(ModelError::InvalidNote)?;
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
    {
        let content = content_mut(contents, primary_content)?;
        content.notes.extend(absorbed);
        crate::model::sort_notes(&mut content.notes);
    }
    let removed: Vec<Id> = retired.iter().map(|(id, _, _)| *id).collect();
    track.clips.retain(|clip| !removed.contains(&clip.id));
    for (_, content, from_origin) in &retired {
        let delta = from_origin
            .checked_sub(primary_origin)
            .ok_or(ModelError::InvalidNote)?;
        retarget(tracks, track, *content, primary_content, delta)?;
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
    delta: i64,
) -> Result<(), ModelError> {
    let edit = |clip: &mut Placement| -> Result<(), ModelError> {
        if clip.content != from {
            return Ok(());
        }
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
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    current: &mut Track,
    content: Id,
    bar: i64,
) -> Result<(), ModelError> {
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
        let mut owned = (*tracks[slot]).clone();
        if owned
            .clips
            .iter()
            .any(|clip| clip.content == content && clip.kind == ClipKind::Auto)
        {
            refit_current(contents, tracks, &mut owned, content, bar)?;
            crate::model::sort_clips(&mut owned.clips);
            // Merge may have retargeted fixed windows on the edited track too.
            for clip in &mut current.clips {
                if let Some(retargeted) = tracks
                    .iter()
                    .find(|track| track.id == current.id)
                    .and_then(|track| track.clips.iter().find(|old| old.id == clip.id))
                    && clip.content != retargeted.content
                {
                    clip.content = retargeted.content;
                    clip.offset = retargeted.offset;
                }
            }
            tracks[slot] = Arc::new(owned);
            return Ok(());
        }
    }
    Ok(())
}

fn refit_current(
    contents: &mut Vec<Arc<Content>>,
    tracks: &mut [Arc<Track>],
    current: &mut Track,
    content: Id,
    bar: i64,
) -> Result<(), ModelError> {
    let index = current
        .clips
        .iter()
        .position(|clip| clip.content == content && clip.kind == ClipKind::Auto)
        .ok_or(ModelError::MissingClip)?;
    let mut span = note_bars(contents, &current.clips[index], bar)?;
    let mut hosts = vec![index];
    loop {
        let before = hosts.len();
        for (slot, clip) in current.clips.iter().enumerate() {
            if clip.kind != ClipKind::Auto || hosts.contains(&slot) {
                continue;
            }
            let candidate = occupied_bars(clip.start.get(), clip.length.get(), bar)
                .ok_or(ModelError::InvalidNote)?;
            if bars_touch(span, candidate) && !fixed_blocks(&current.clips, span, candidate, bar) {
                hosts.push(slot);
                span = (span.0.min(candidate.0), span.1.max(candidate.1));
            }
        }
        if before == hosts.len() {
            break;
        }
    }
    if hosts.len() == 1 {
        refit_one(contents, &mut current.clips, index, bar)
    } else {
        merge(contents, tracks, current, &hosts, bar)
    }
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
    for note in &found {
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
