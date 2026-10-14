//! Piano geometry uses the same immutable content and placement indexes as arrange.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PianoView {
    pub origin_tick: Tick,
    pub width_ticks: i64,
    /// Pitch at the top edge; a row occupies [pitch, pitch + 1) in screen order.
    pub top_pitch: f64,
    pub height_pitches: f64,
    pub pixels_per_tick: f64,
    pub pixels_per_pitch: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PianoNote {
    pub id: ElementId,
    /// Full timeline geometry, before clipping by the placement or viewport.
    pub start: Tick,
    pub length: Tick,
    pub pitch: u8,
    pub velocity: u16,
    pub window_start: Tick,
    pub window_end: Tick,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PianoView {
    fn usable(self) -> bool {
        self.width_ticks > 0
            && self
                .origin_tick
                .get()
                .checked_add(self.width_ticks)
                .is_some()
            && self.top_pitch.is_finite()
            && self.height_pitches.is_finite()
            && self.height_pitches > 0.
            && self.pixels_per_tick.is_finite()
            && self.pixels_per_tick > 0.
            && self.pixels_per_pitch.is_finite()
            && self.pixels_per_pitch > 0.
    }
}

impl ArrangeIndex {
    /// Visits only time intervals and pitch rows intersecting the viewport.
    /// This does not allocate or walk notes on hidden pitches.
    pub fn visit_piano(
        &self,
        project: &Project,
        track: Id,
        view: PianoView,
        mut emit: impl FnMut(PianoNote),
    ) {
        if !view.usable() {
            return;
        }
        let Some(&slot) = self.tracks.get(&track) else {
            return;
        };
        let track = &project.tracks()[slot];
        let origin = view.origin_tick.get();
        let end = origin + view.width_ticks;
        self.placements[slot].ends.visit(
            origin,
            end,
            |i| track.clips[i].start.get(),
            |i| {
                let clip = &track.clips[i];
                let Some(index) = self.piano_content(project, clip.content) else {
                    return;
                };
                let Some((from, to)) = content_window(clip, origin, end) else {
                    return;
                };
                let low = view.top_pitch - view.height_pitches;
                for row in &index.pitches {
                    let pitch = f64::from(row.pitch);
                    if pitch >= view.top_pitch + 1. || pitch <= low {
                        continue;
                    }
                    row.ends.visit(
                        from,
                        to,
                        |n| index.content.notes[row.notes[n]].start.get(),
                        |n| {
                            let note = &index.content.notes[row.notes[n]];
                            let Some((start, finish)) = audible_in_view(clip, note, origin, end)
                            else {
                                return;
                            };
                            let Some(mut item) = piano_note(track.id, clip, note) else {
                                return;
                            };
                            item.x = ((start as f64 - origin as f64) * view.pixels_per_tick) as f32;
                            item.y = ((view.top_pitch - pitch) * view.pixels_per_pitch) as f32;
                            item.w = ((finish as f64 - start as f64) * view.pixels_per_tick) as f32;
                            item.h = view.pixels_per_pitch as f32;
                            emit(item);
                        },
                    );
                }
            },
        );
    }

    /// Resolves a focus after edits. A merge may retire the original placement;
    /// the unchanged note ID then resolves in its surviving window on this track.
    pub fn piano_note(
        &self,
        project: &Project,
        track: Id,
        placement: Option<Id>,
        note: Id,
    ) -> Option<PianoNote> {
        let content_id = *self.note_content.get(&note)?;
        let content = self.piano_content(project, content_id)?;
        let note_index = *content.by_id.get(&note)?;
        let note = content.content.notes.get(note_index)?;
        let &slot = self.tracks.get(&track)?;
        let mut fallback = None;
        for clip in project.tracks().get(slot)?.clips.iter() {
            if clip.content != content_id {
                continue;
            }
            let Some(item) = piano_note(track, clip, note) else {
                continue;
            };
            if item.id.placement == placement {
                return Some(item);
            }
            if fallback.is_none() {
                fallback = Some(item);
            }
        }
        fallback
    }

    /// Time, pitch, placement and note ID give deterministic navigation through
    /// all audible notes, including notes outside the current viewport.
    pub fn piano_neighbor(
        &self,
        project: &Project,
        track: Id,
        current: Option<ElementId>,
        backwards: bool,
    ) -> Option<PianoNote> {
        let key = current
            .and_then(|id| self.piano_note(project, track, id.placement, id.note?))
            .map(order);
        let mut found: Option<PianoNote> = None;
        self.each_piano(project, track, |item| {
            let candidate = order(item);
            let eligible = key.is_none_or(|key| {
                if backwards {
                    candidate < key
                } else {
                    candidate > key
                }
            });
            if eligible
                && found.is_none_or(|old| {
                    if backwards {
                        candidate > order(old)
                    } else {
                        candidate < order(old)
                    }
                })
            {
                found = Some(item);
            }
        });
        found
    }

    fn piano_content(&self, project: &Project, id: Id) -> Option<&ContentIndex> {
        let slot = self
            .contents
            .binary_search_by_key(&id, |item| item.id())
            .ok()?;
        let index = &self.contents[slot];
        let current = project
            .contents()
            .binary_search_by_key(&id, |item| item.id)
            .ok()?;
        Arc::ptr_eq(&index.content, &project.contents()[current]).then_some(index)
    }

    // Full traversal is restricted to explicit focus/navigation requests.
    fn each_piano(&self, project: &Project, track: Id, mut emit: impl FnMut(PianoNote)) {
        let Some(&slot) = self.tracks.get(&track) else {
            return;
        };
        for clip in &project.tracks()[slot].clips {
            let Some(content) = self.piano_content(project, clip.content) else {
                continue;
            };
            for note in &content.content.notes {
                if let Some(item) = piano_note(track, clip, note) {
                    emit(item);
                }
            }
        }
    }
}

fn piano_note(track: Id, clip: &Placement, note: &Note) -> Option<PianoNote> {
    clip.audible(note).ok().flatten()?;
    Some(PianoNote {
        id: ElementId {
            role: Role::Note,
            track,
            placement: Some(clip.id),
            note: Some(note.id),
        },
        start: clip.sounding_start(note)?,
        length: note.length,
        pitch: note.pitch,
        velocity: note.velocity,
        window_start: clip.start,
        window_end: Tick::new(clip.start.get().checked_add(clip.length.get())?),
        x: 0.,
        y: 0.,
        w: 0.,
        h: 0.,
    })
}

fn order(note: PianoNote) -> (Tick, u8, Option<Id>, Option<Id>) {
    (note.start, note.pitch, note.id.placement, note.id.note)
}
