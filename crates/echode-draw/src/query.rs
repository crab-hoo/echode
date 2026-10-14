//! Visible arrange elements from a project snapshot.
//!
//! The interval tree is built when the snapshot changes. A frame only walks
//! branches that can intersect the view and writes into the caller's buffer.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[path = "piano.rs"]
mod piano;
pub use piano::{PianoNote, PianoView};

use echode_model::{Content, Id, Note, Placement, Project, Tick, Track};

/// What a visible element is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Track,
    Clip,
    Note,
}

/// Identity of a track, placement, or note. A note keeps its placement because
/// one content can sit in several windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElementId {
    pub role: Role,
    pub track: Id,
    pub placement: Option<Id>,
    pub note: Option<Id>,
}

/// One visible object and its pixel rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Element {
    pub id: ElementId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Arrange viewport. Time and track ranges are half-open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrangeView {
    pub origin_tick: Tick,
    pub width_ticks: i64,
    pub origin_track: f64,
    pub height_tracks: f64,
    pub pixels_per_tick: f64,
    pub pixels_per_track: f64,
}

/// Note rectangles leave the top and bottom of the lane free so the placement
/// underneath can still be hit.
const NOTE_INSET: f64 = 0.14;
const NOTE_HEIGHT: f64 = 0.72;

/// Indexes reused across project versions. Querying does not allocate.
#[derive(Clone, Debug)]
pub struct ArrangeIndex {
    contents: Vec<Arc<ContentIndex>>,
    placements: Vec<Arc<PlacementIndex>>,
    tracks: BTreeMap<Id, usize>,
    /// Note id to content id. Shared across updates that reuse every content.
    note_content: Arc<BTreeMap<Id, Id>>,
}

#[derive(Debug)]
struct ContentIndex {
    content: Arc<Content>,
    ends: MaxEnd,
    pitches: Vec<PitchIndex>,
    by_id: BTreeMap<Id, usize>,
}

#[derive(Debug)]
struct PitchIndex {
    pitch: u8,
    notes: Vec<usize>,
    ends: MaxEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PlacementTime {
    id: Id,
    start: i64,
    length: i64,
    offset: i64,
}

#[derive(Debug)]
struct PlacementIndex {
    track: Id,
    times: Vec<PlacementTime>,
    ends: MaxEnd,
}

/// Complete binary tree of maximum end points. Leaves follow start order.
#[derive(Clone, Debug)]
struct MaxEnd {
    len: usize,
    size: usize,
    tree: Vec<i64>,
}

impl MaxEnd {
    fn build(ends: &[i64]) -> Self {
        let len = ends.len();
        if len == 0 {
            return Self {
                len: 0,
                size: 0,
                tree: Vec::new(),
            };
        }
        let size = len.next_power_of_two();
        let mut tree = vec![i64::MIN; size * 2];
        for (index, end) in ends.iter().enumerate() {
            tree[size + index] = *end;
        }
        for index in (1..size).rev() {
            tree[index] = tree[index * 2].max(tree[index * 2 + 1]);
        }
        Self { len, size, tree }
    }

    /// Item `i` overlaps when `start_of(i) < query_end` and its stored end is `> query_start`.
    fn visit<S, F>(&self, query_start: i64, query_end: i64, start_of: S, mut emit: F) -> u64
    where
        S: Fn(usize) -> i64,
        F: FnMut(usize),
    {
        if self.len == 0 || query_start >= query_end {
            return 0;
        }
        let upper = upper_bound(self.len, &start_of, query_end);
        let mut visits = 0;
        let mut walk = Walk {
            query_start,
            query_end,
            upper,
            start_of: &start_of,
            visits: &mut visits,
            emit: &mut emit,
        };
        self.walk(1, 0, self.size, &mut walk);
        visits
    }

    fn walk<S, F>(&self, node: usize, low: usize, high: usize, walk: &mut Walk<'_, S, F>)
    where
        S: Fn(usize) -> i64,
        F: FnMut(usize),
    {
        if low >= walk.upper {
            return;
        }
        *walk.visits = walk.visits.saturating_add(1);
        if self.tree[node] <= walk.query_start {
            return;
        }
        if high - low == 1 {
            let start = (walk.start_of)(low);
            let end = self.tree[self.size + low];
            if start < walk.query_end && end > walk.query_start {
                (walk.emit)(low);
            }
            return;
        }
        let mid = low + (high - low) / 2;
        self.walk(node * 2, low, mid, walk);
        self.walk(node * 2 + 1, mid, high, walk);
    }
}

struct Walk<'a, S, F> {
    query_start: i64,
    query_end: i64,
    upper: usize,
    start_of: &'a S,
    visits: &'a mut u64,
    emit: &'a mut F,
}

fn upper_bound(len: usize, start_of: &impl Fn(usize) -> i64, query_end: i64) -> usize {
    let mut low = 0;
    let mut high = len;
    while low < high {
        let mid = low + (high - low) / 2;
        if start_of(mid) < query_end {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

impl ArrangeIndex {
    pub fn build(project: &Project) -> Self {
        let contents: Vec<_> = project
            .contents()
            .iter()
            .map(|content| Arc::new(ContentIndex::build(Arc::clone(content))))
            .collect();
        Self {
            tracks: project
                .tracks()
                .iter()
                .enumerate()
                .map(|(i, t)| (t.id, i))
                .collect(),
            note_content: Arc::new(note_content_of(&contents)),
            contents,
            placements: project
                .tracks()
                .iter()
                .map(|track| Arc::new(PlacementIndex::build(track)))
                .collect(),
        }
    }

    /// Keeps trees whose content arc, or placement time intervals, did not change.
    pub fn update(&self, project: &Project) -> Self {
        let mut contents = Vec::with_capacity(project.contents().len());
        let mut previous = 0;
        let mut reused_contents = true;
        for content in project.contents() {
            while previous < self.contents.len() && self.contents[previous].id() < content.id {
                previous += 1;
            }
            if previous < self.contents.len()
                && self.contents[previous].id() == content.id
                && Arc::ptr_eq(&self.contents[previous].content, content)
            {
                contents.push(Arc::clone(&self.contents[previous]));
            } else {
                reused_contents = false;
                contents.push(Arc::new(ContentIndex::build(Arc::clone(content))));
            }
        }
        if contents.len() != self.contents.len() {
            reused_contents = false;
        }
        let previous_placements: HashMap<Id, Arc<PlacementIndex>> = self
            .placements
            .iter()
            .map(|index| (index.track, Arc::clone(index)))
            .collect();
        let placements = project
            .tracks()
            .iter()
            .map(|track| match previous_placements.get(&track.id) {
                Some(index) if index.same_track(track) => Arc::clone(index),
                _ => Arc::new(PlacementIndex::build(track)),
            })
            .collect();
        let note_content = if reused_contents {
            Arc::clone(&self.note_content)
        } else {
            Arc::new(note_content_of(&contents))
        };
        Self {
            contents,
            placements,
            tracks: project
                .tracks()
                .iter()
                .enumerate()
                .map(|(i, t)| (t.id, i))
                .collect(),
            note_content,
        }
    }

    /// Writes elements that intersect `view`. The second number is how many did not fit.
    pub fn visible(&self, project: &Project, view: ArrangeView, out: &mut [Element]) -> (u32, u32) {
        let mut written = 0usize;
        let mut dropped = 0u32;
        self.each_visible(project, view, |element| {
            if written < out.len() {
                out[written] = element;
                written += 1;
            } else {
                dropped = dropped.saturating_add(1);
            }
        });
        (written as u32, dropped)
    }

    /// Every element that intersects `view`, including ones past a short buffer.
    pub fn visit_visible(&self, project: &Project, view: ArrangeView, emit: impl FnMut(Element)) {
        self.each_visible(project, view, emit);
    }

    /// The front-most element whose rectangle contains the point.
    pub fn hit(&self, project: &Project, view: ArrangeView, x: f32, y: f32) -> Option<ElementId> {
        let mut found = None;
        self.each_visible(project, view, |element| {
            if contains(element, x, y) {
                found = Some(element.id);
            }
        });
        found
    }

    /// The next object in arrange order, including ones outside the view.
    pub fn logical_next(&self, project: &Project, current: Option<ElementId>) -> Option<ElementId> {
        let mut armed = current.is_none();
        let mut found = None;
        each_logical(project, |id| {
            if armed {
                found = Some(id);
                return false;
            }
            if Some(id) == current {
                armed = true;
            }
            true
        });
        found
    }

    /// The previous object in arrange order.
    pub fn logical_prev(&self, project: &Project, current: Option<ElementId>) -> Option<ElementId> {
        let mut previous = None;
        each_logical(project, |id| {
            if Some(id) == current {
                return false;
            }
            previous = Some(id);
            true
        });
        previous
    }

    /// Keeps `focus` when that object is still in the project. Otherwise steps
    /// up to the placement, the track, or the first track.
    pub fn retain(&self, project: &Project, focus: ElementId) -> Option<ElementId> {
        if exists(project, focus) {
            return Some(focus);
        }
        let track_id = focus.track;
        let track_ok = project.tracks().iter().any(|track| track.id == track_id);
        match focus.role {
            Role::Note => {
                if let Some(placement) = focus.placement.filter(|_| {
                    track_ok && placement_on(project, track_id, focus.placement).is_some()
                }) {
                    Some(clip_id(track_id, placement))
                } else if track_ok {
                    Some(track_only(track_id))
                } else {
                    first_track(project)
                }
            }
            Role::Clip if track_ok => Some(track_only(track_id)),
            Role::Clip | Role::Track => first_track(project),
        }
    }

    /// Moves the origin so `focus` intersects the view, when the object exists.
    pub fn reveal(&self, project: &Project, view: ArrangeView, focus: ElementId) -> ArrangeView {
        let Some((track_index, start, end)) = span(project, focus) else {
            return view;
        };
        if !view.usable() {
            return view;
        }
        let mut revealed = view;
        if focus.role != Role::Track {
            revealed.origin_tick =
                Tick::new(cover(view.origin_tick.get(), view.width_ticks, start, end));
        }
        revealed.origin_track = cover_f64(
            view.origin_track,
            view.height_tracks,
            track_index as f64,
            track_index as f64 + 1.0,
        );
        revealed
    }

    fn each_visible(&self, project: &Project, view: ArrangeView, mut emit: impl FnMut(Element)) {
        let Some((first, last)) = visible_tracks(view, project.tracks().len()) else {
            return;
        };
        let origin = view.origin_tick.get();
        let view_end = match origin.checked_add(view.width_ticks) {
            Some(end) => end,
            None => return,
        };
        for track_index in first..last {
            let track = &project.tracks()[track_index];
            emit(track_element(track.id, track_index, &view));
            let Some(&slot) = self.tracks.get(&track.id) else {
                continue;
            };
            let track_id = track.id;
            self.placements[slot].ends.visit(
                origin,
                view_end,
                |clip_index| track.clips[clip_index].start.get(),
                |clip_index| {
                    let clip = &track.clips[clip_index];
                    let Some(clip_end) = clip.start.get().checked_add(clip.length.get()) else {
                        return;
                    };
                    let shown_start = origin.max(clip.start.get());
                    let shown_end = view_end.min(clip_end);
                    emit(clip_element(
                        track_id,
                        clip.id,
                        track_index,
                        shown_start,
                        shown_end,
                        &view,
                    ));
                    NoteQuery {
                        contents: &self.contents,
                        project,
                        track: track_id,
                        clip,
                        track_index,
                        origin,
                        view_end,
                        view: &view,
                    }
                    .emit(&mut emit);
                },
            );
        }
    }

    #[cfg(test)]
    fn note_visits(&self, content: &Content, query_start: i64, query_end: i64) -> (u32, u64) {
        let Some(index) = self.contents.iter().find(|index| index.id() == content.id) else {
            return (0, 0);
        };
        let mut hits = 0u32;
        let visits = index.ends.visit(
            query_start,
            query_end,
            |note_index| content.notes[note_index].start.get(),
            |_| hits = hits.saturating_add(1),
        );
        (hits, visits)
    }

    #[cfg(test)]
    fn shares_content(&self, other: &Self) -> bool {
        self.contents.len() == other.contents.len()
            && self
                .contents
                .iter()
                .zip(&other.contents)
                .all(|(left, right)| Arc::ptr_eq(left, right))
    }

    #[cfg(test)]
    fn shares_placement(&self, other: &Self, track: Id) -> bool {
        match (
            self.placements.iter().find(|index| index.track == track),
            other.placements.iter().find(|index| index.track == track),
        ) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl ContentIndex {
    fn build(content: Arc<Content>) -> Self {
        let ends: Vec<i64> = content
            .notes
            .iter()
            .map(|note| range_end(note.start.get(), note.length.get()))
            .collect();
        let mut by_pitch: BTreeMap<u8, Vec<usize>> = BTreeMap::new();
        for (index, note) in content.notes.iter().enumerate() {
            by_pitch.entry(note.pitch).or_default().push(index);
        }
        let pitches = by_pitch
            .into_iter()
            .map(|(pitch, notes)| {
                let ends: Vec<_> = notes
                    .iter()
                    .map(|&i| {
                        range_end(content.notes[i].start.get(), content.notes[i].length.get())
                    })
                    .collect();
                PitchIndex {
                    pitch,
                    notes,
                    ends: MaxEnd::build(&ends),
                }
            })
            .collect();
        let mut by_id = BTreeMap::new();
        for (index, note) in content.notes.iter().enumerate() {
            by_id.insert(note.id, index);
        }
        Self {
            content,
            ends: MaxEnd::build(&ends),
            pitches,
            by_id,
        }
    }

    fn id(&self) -> Id {
        self.content.id
    }
}

impl PlacementIndex {
    fn build(track: &Track) -> Self {
        let times: Vec<PlacementTime> = track.clips.iter().map(PlacementTime::from).collect();
        let ends: Vec<i64> = times
            .iter()
            .map(|time| range_end(time.start, time.length))
            .collect();
        Self {
            track: track.id,
            times,
            ends: MaxEnd::build(&ends),
        }
    }

    fn same_track(&self, track: &Track) -> bool {
        self.track == track.id
            && self.times.len() == track.clips.len()
            && self
                .times
                .iter()
                .zip(&track.clips)
                .all(|(time, clip)| *time == PlacementTime::from(clip))
    }
}

impl PlacementTime {
    fn from(clip: &Placement) -> Self {
        Self {
            id: clip.id,
            start: clip.start.get(),
            length: clip.length.get(),
            offset: clip.offset.get(),
        }
    }
}

fn range_end(start: i64, length: i64) -> i64 {
    start.checked_add(length).unwrap_or(start)
}

struct NoteQuery<'a> {
    contents: &'a [Arc<ContentIndex>],
    project: &'a Project,
    track: Id,
    clip: &'a Placement,
    track_index: usize,
    origin: i64,
    view_end: i64,
    view: &'a ArrangeView,
}

fn note_content_of(contents: &[Arc<ContentIndex>]) -> BTreeMap<Id, Id> {
    let mut notes = BTreeMap::new();
    for content in contents {
        for note in &content.content.notes {
            notes.insert(note.id, content.id());
        }
    }
    notes
}

impl NoteQuery<'_> {
    fn emit(&self, emit: &mut impl FnMut(Element)) {
        let Ok(slot) = self
            .contents
            .binary_search_by_key(&self.clip.content, |index| index.id())
        else {
            return;
        };
        let Some(index) = self.contents.get(slot) else {
            return;
        };
        let Ok(current) = self
            .project
            .contents()
            .binary_search_by_key(&self.clip.content, |content| content.id)
        else {
            return;
        };
        let Some(current) = self.project.contents().get(current) else {
            return;
        };
        if !Arc::ptr_eq(&index.content, current) {
            return;
        }
        let Some((query_start, query_end)) = content_window(self.clip, self.origin, self.view_end)
        else {
            return;
        };
        let ends = &index.ends;
        let notes = &index.content.notes;
        ends.visit(
            query_start,
            query_end,
            |note_index| notes[note_index].start.get(),
            |note_index| {
                let note = &notes[note_index];
                let Some((start, end)) =
                    audible_in_view(self.clip, note, self.origin, self.view_end)
                else {
                    return;
                };
                emit(note_element(
                    self.track,
                    self.clip.id,
                    note.id,
                    self.track_index,
                    start,
                    end,
                    self.view,
                ));
            },
        );
    }
}

fn content_window(clip: &Placement, origin: i64, view_end: i64) -> Option<(i64, i64)> {
    let clip_end = clip.start.get().checked_add(clip.length.get())?;
    let shown_start = origin.max(clip.start.get());
    let shown_end = view_end.min(clip_end);
    if shown_start >= shown_end {
        return None;
    }
    let shift = clip.offset.get().checked_sub(clip.start.get())?;
    Some((
        shown_start.checked_add(shift)?,
        shown_end.checked_add(shift)?,
    ))
}

fn audible_in_view(
    clip: &Placement,
    note: &Note,
    origin: i64,
    view_end: i64,
) -> Option<(i64, i64)> {
    let (start, end) = clip.audible(note).ok().flatten()?;
    let shown_start = origin.max(start.get());
    let shown_end = view_end.min(end.get());
    if shown_start >= shown_end {
        None
    } else {
        Some((shown_start, shown_end))
    }
}

fn visible_tracks(view: ArrangeView, count: usize) -> Option<(usize, usize)> {
    if !view.usable() || count == 0 {
        return None;
    }
    let start = view.origin_track;
    let end = start + view.height_tracks;
    if end <= 0.0 {
        return None;
    }
    let first = if start <= 0.0 {
        0
    } else {
        start.floor() as usize
    };
    let last = (end.ceil() as usize).min(count);
    if first >= last || first >= count {
        None
    } else {
        Some((first, last))
    }
}

impl ArrangeView {
    fn usable(self) -> bool {
        self.width_ticks > 0
            && self.height_tracks > 0.0
            && self.pixels_per_tick.is_finite()
            && self.pixels_per_tick > 0.0
            && self.pixels_per_track.is_finite()
            && self.pixels_per_track > 0.0
            && self.origin_track.is_finite()
            && self.height_tracks.is_finite()
    }
}

fn track_element(track: Id, index: usize, view: &ArrangeView) -> Element {
    let y = (index as f64 - view.origin_track) * view.pixels_per_track;
    let height = view.pixels_per_track;
    let width = view.width_ticks as f64 * view.pixels_per_tick;
    Element {
        id: track_only(track),
        x: 0.0,
        y: y as f32,
        w: width as f32,
        h: height as f32,
    }
}

fn clip_element(
    track: Id,
    placement: Id,
    index: usize,
    start: i64,
    end: i64,
    view: &ArrangeView,
) -> Element {
    let y = (index as f64 - view.origin_track) * view.pixels_per_track;
    Element {
        id: clip_id(track, placement),
        x: tick_x(start, view) as f32,
        y: y as f32,
        w: ((end - start) as f64 * view.pixels_per_tick) as f32,
        h: view.pixels_per_track as f32,
    }
}

fn note_element(
    track: Id,
    placement: Id,
    note: Id,
    index: usize,
    start: i64,
    end: i64,
    view: &ArrangeView,
) -> Element {
    let y = (index as f64 - view.origin_track) * view.pixels_per_track;
    Element {
        id: ElementId {
            role: Role::Note,
            track,
            placement: Some(placement),
            note: Some(note),
        },
        x: tick_x(start, view) as f32,
        y: (y + view.pixels_per_track * NOTE_INSET) as f32,
        w: ((end - start) as f64 * view.pixels_per_tick) as f32,
        h: (view.pixels_per_track * NOTE_HEIGHT) as f32,
    }
}

fn tick_x(tick: i64, view: &ArrangeView) -> f64 {
    (tick - view.origin_tick.get()) as f64 * view.pixels_per_tick
}

fn track_only(track: Id) -> ElementId {
    ElementId {
        role: Role::Track,
        track,
        placement: None,
        note: None,
    }
}

fn clip_id(track: Id, placement: Id) -> ElementId {
    ElementId {
        role: Role::Clip,
        track,
        placement: Some(placement),
        note: None,
    }
}

fn contains(element: Element, x: f32, y: f32) -> bool {
    if !element.x.is_finite()
        || !element.y.is_finite()
        || !element.w.is_finite()
        || !element.h.is_finite()
        || element.w <= 0.0
        || element.h <= 0.0
    {
        return false;
    }
    x >= element.x && y >= element.y && x < element.x + element.w && y < element.y + element.h
}

fn each_logical(project: &Project, mut visit: impl FnMut(ElementId) -> bool) {
    for track in project.tracks() {
        if !visit(track_only(track.id)) {
            return;
        }
        for clip in &track.clips {
            if !visit(clip_id(track.id, clip.id)) {
                return;
            }
            let Some(content) = project.content(clip.content) else {
                continue;
            };
            for note in &content.notes {
                if clip.audible(note).ok().flatten().is_none() {
                    continue;
                }
                let id = ElementId {
                    role: Role::Note,
                    track: track.id,
                    placement: Some(clip.id),
                    note: Some(note.id),
                };
                if !visit(id) {
                    return;
                }
            }
        }
    }
}

fn exists(project: &Project, focus: ElementId) -> bool {
    let Some(track) = project
        .tracks()
        .iter()
        .find(|track| track.id == focus.track)
    else {
        return false;
    };
    match focus.role {
        Role::Track => true,
        Role::Clip => track
            .clips
            .iter()
            .any(|clip| Some(clip.id) == focus.placement),
        Role::Note => {
            let Some(clip) = placement_on(project, track.id, focus.placement) else {
                return false;
            };
            let Some(content) = project.content(clip.content) else {
                return false;
            };
            content.notes.iter().any(|note| {
                Some(note.id) == focus.note && clip.audible(note).ok().flatten().is_some()
            })
        }
    }
}

fn placement_on(project: &Project, track: Id, placement: Option<Id>) -> Option<&Placement> {
    project
        .tracks()
        .iter()
        .find(|candidate| candidate.id == track)?
        .clips
        .iter()
        .find(|clip| Some(clip.id) == placement)
}

fn first_track(project: &Project) -> Option<ElementId> {
    project.tracks().first().map(|track| track_only(track.id))
}

fn span(project: &Project, focus: ElementId) -> Option<(usize, i64, i64)> {
    let track_index = project
        .tracks()
        .iter()
        .position(|track| track.id == focus.track)?;
    let track = &project.tracks()[track_index];
    match focus.role {
        Role::Track => Some((track_index, 0, 0)),
        Role::Clip => {
            let clip = track
                .clips
                .iter()
                .find(|clip| Some(clip.id) == focus.placement)?;
            let end = clip.start.get().checked_add(clip.length.get())?;
            Some((track_index, clip.start.get(), end))
        }
        Role::Note => {
            let clip = placement_on(project, focus.track, focus.placement)?;
            let content = project.content(clip.content)?;
            let note = content
                .notes
                .iter()
                .find(|note| Some(note.id) == focus.note)?;
            let (start, end) = clip.audible(note).ok().flatten()?;
            Some((track_index, start.get(), end.get()))
        }
    }
}

fn cover(origin: i64, width: i64, start: i64, end: i64) -> i64 {
    if start >= origin && end <= origin.saturating_add(width) {
        return origin;
    }
    if end.saturating_sub(start) >= width {
        return start;
    }
    if start < origin {
        return start;
    }
    end.saturating_sub(width)
}

fn cover_f64(origin: f64, span: f64, start: f64, end: f64) -> f64 {
    if start >= origin && end <= origin + span {
        return origin;
    }
    if end - start >= span {
        return start;
    }
    if start < origin {
        return start;
    }
    end - span
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_model::{
        ClipKind, Content, DEFAULT_FADE_FLICKS, Lane, Meter, Note, NoteDraft, Placement, Project,
        TICKS_PER_QUARTER, Tempo, TempoMap, Tick, Track, initial_order, midi7_to_velocity,
        test_mixer,
    };

    fn view_of(origin: i64, width: i64, tracks: f64) -> ArrangeView {
        ArrangeView {
            origin_tick: Tick::new(origin),
            width_ticks: width,
            origin_track: 0.0,
            height_tracks: tracks,
            pixels_per_tick: 1.0,
            pixels_per_track: 20.0,
        }
    }

    fn piano() -> (Project, Id) {
        Project::new().add_track("piano")
    }

    fn quarter(start_quarters: i64, length_quarters: i64) -> NoteDraft {
        NoteDraft {
            pitch: 60,
            velocity: 100,
            start: Tick::new(start_quarters * TICKS_PER_QUARTER),
            length: Tick::new(length_quarters * TICKS_PER_QUARTER),
        }
    }

    #[test]
    fn a_note_that_starts_before_the_view_is_visible_until_it_ends() {
        let (project, track) = piano();
        let (project, long) = project.insert_note(track, quarter(0, 20)).unwrap();
        let (project, early) = project.insert_note(track, quarter(0, 1)).unwrap();
        let index = ArrangeIndex::build(&project);
        let mut out = [Element {
            id: track_only(track),
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        }; 8];
        let (written, dropped) = index.visible(
            &project,
            view_of(10 * TICKS_PER_QUARTER, TICKS_PER_QUARTER, 1.0),
            &mut out,
        );
        assert_eq!(dropped, 0);
        let notes: Vec<_> = out[..written as usize]
            .iter()
            .filter(|element| element.id.role == Role::Note)
            .map(|element| element.id.note)
            .collect();
        assert_eq!(notes, vec![Some(long)]);
        assert!(!notes.contains(&Some(early)));
    }

    #[test]
    fn a_note_outside_the_placement_window_is_hidden() {
        let (project, track) = piano();
        let (project, outside) = project.insert_note(track, quarter(0, 1)).unwrap();
        let (project, inside) = project.insert_note(track, quarter(1, 1)).unwrap();
        let clip = project.tracks()[0].clips[0].id;
        let project = project
            .resize_clip(
                track,
                clip,
                Tick::new(TICKS_PER_QUARTER),
                Tick::new(TICKS_PER_QUARTER),
            )
            .unwrap();
        let index = ArrangeIndex::build(&project);
        let mut out = [Element {
            id: track_only(track),
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        }; 8];
        let (written, _) =
            index.visible(&project, view_of(0, 20 * TICKS_PER_QUARTER, 1.0), &mut out);
        let notes: Vec<_> = out[..written as usize]
            .iter()
            .filter(|element| element.id.role == Role::Note)
            .map(|element| element.id.note)
            .collect();
        assert_eq!(notes, vec![Some(inside)]);
        assert!(!notes.contains(&Some(outside)));
    }

    #[test]
    fn one_note_in_two_placements_is_two_elements() {
        let (project, track) = piano();
        let (project, note) = project.insert_note(track, quarter(0, 1)).unwrap();
        let (project, other) = project.add_track("copy");
        let source = project.tracks()[0].clips[0].id;
        let link = Id::from_raw(42);
        let project = project
            .link_clip(source, other, Tick::new(0), link)
            .unwrap();
        let index = ArrangeIndex::build(&project);
        let mut out = [Element {
            id: track_only(track),
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        }; 16];
        let (written, dropped) =
            index.visible(&project, view_of(0, 4 * TICKS_PER_QUARTER, 2.0), &mut out);
        assert_eq!(dropped, 0);
        let notes: Vec<_> = out[..written as usize]
            .iter()
            .filter(|element| element.id.role == Role::Note)
            .map(|element| (element.id.placement, element.id.note))
            .collect();
        assert_eq!(
            notes,
            vec![(Some(source), Some(note)), (Some(link), Some(note))]
        );
    }

    #[test]
    fn a_short_buffer_counts_the_overflow_and_a_hit_prefers_the_note() {
        let (project, track) = piano();
        let (project, first) = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    ..quarter(0, 1)
                },
            )
            .unwrap();
        let (project, second) = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 72,
                    ..quarter(0, 1)
                },
            )
            .unwrap();
        let index = ArrangeIndex::build(&project);
        let mut one = [Element {
            id: track_only(track),
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        }; 1];
        let (written, dropped) =
            index.visible(&project, view_of(0, TICKS_PER_QUARTER, 1.0), &mut one);
        assert_eq!(written, 1);
        assert!(dropped >= 1);
        let view = view_of(0, TICKS_PER_QUARTER, 1.0);
        let hit = index
            .hit(&project, view, (TICKS_PER_QUARTER / 2) as f32, 10.0)
            .unwrap();
        assert_eq!(hit.role, Role::Note);
        assert_eq!(hit.note, Some(second));
        assert_ne!(hit.note, Some(first));
        let lane = index
            .hit(&project, view, (TICKS_PER_QUARTER / 2) as f32, 1.0)
            .unwrap();
        assert_eq!(lane.role, Role::Clip);
    }

    #[test]
    fn logical_order_leaves_the_view_and_focus_survives_a_scroll() {
        let (project, track) = piano();
        let (project, early) = project.insert_note(track, quarter(0, 1)).unwrap();
        let (project, late) = project.insert_note(track, quarter(1, 1)).unwrap();
        let index = ArrangeIndex::build(&project);
        let view = view_of(0, TICKS_PER_QUARTER, 1.0);
        let mut out = [Element {
            id: track_only(track),
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        }; 8];
        let (written, _) = index.visible(&project, view, &mut out);
        let visible_notes: Vec<_> = out[..written as usize]
            .iter()
            .filter(|element| element.id.role == Role::Note)
            .map(|element| element.id.note)
            .collect();
        assert_eq!(visible_notes, vec![Some(early)]);
        let early_id = out
            .iter()
            .find(|element| element.id.note == Some(early))
            .unwrap()
            .id;
        let next = index.logical_next(&project, Some(early_id)).unwrap();
        assert_eq!(next.note, Some(late));
        assert!(index.retain(&project, next).unwrap().note == Some(late));
        let revealed = index.reveal(&project, view, next);
        let (written, _) = index.visible(&project, revealed, &mut out);
        let shown: Vec<_> = out[..written as usize]
            .iter()
            .filter(|element| element.id.role == Role::Note)
            .map(|element| element.id.note)
            .collect();
        assert_eq!(shown, vec![Some(late)]);
    }

    #[test]
    fn a_deleted_note_keeps_focus_on_its_placement() {
        let (project, track) = piano();
        let (project, keep) = project.insert_note(track, quarter(0, 1)).unwrap();
        let (project, gone) = project.insert_note(track, quarter(1, 1)).unwrap();
        let clip = project.tracks()[0].clips[0].id;
        let focus = ElementId {
            role: Role::Note,
            track,
            placement: Some(clip),
            note: Some(gone),
        };
        let index = ArrangeIndex::build(&project);
        let project = project.delete_note(track, gone).unwrap();
        let index = index.update(&project);
        let kept = index.retain(&project, focus).unwrap();
        assert_eq!(kept.role, Role::Clip);
        assert_eq!(kept.placement, Some(clip));
        assert!(
            index
                .logical_next(&project, Some(track_only(track)))
                .is_some()
        );
        let _ = keep;
    }

    #[test]
    fn an_unrelated_edit_reuses_the_content_and_placement_trees() {
        let (project, track) = piano();
        let (project, _) = project.insert_note(track, quarter(0, 4)).unwrap();
        let index = ArrangeIndex::build(&project);
        let gain = project.tracks()[0].devices[0].parameters[0].id;
        let quieter = project.set_parameter(gain, 0.5).unwrap();
        let updated = index.update(&quieter);
        assert!(index.shares_content(&updated));
        assert!(index.shares_placement(&updated, track));
        let clip = project.tracks()[0].clips[0].id;
        let resized = project
            .resize_clip(track, clip, Tick::new(0), Tick::new(2 * TICKS_PER_QUARTER))
            .unwrap();
        let moved = index.update(&resized);
        assert!(index.shares_content(&moved));
        assert!(!index.shares_placement(&moved, track));
    }

    #[test]
    fn sparse_long_notes_do_not_walk_the_whole_content() {
        // This population is the UI-1 check for a sparse overlap, not a product limit.
        const NOTES: usize = 1 << 20;
        const LONG: usize = 1024;
        let window = (NOTES as i64 - 1) * 2 + 1;
        let mut notes = Vec::with_capacity(NOTES);
        for index in 0..NOTES {
            let start = index as i64 * 2;
            let long = index % (NOTES / LONG) == 0;
            let length = if long { window + 1 - start } else { 1 };
            notes.push(Note {
                id: Id::from_raw(1_000_000 + index as u128),
                start: Tick::new(start),
                length: Tick::new(length),
                pitch: 60,
                velocity: midi7_to_velocity(100),
                release: 0,
                articulation: None,
            });
        }
        let content_id = Id::from_raw(11);
        let track_id = Id::from_raw(12);
        let clip_id = Id::from_raw(13);
        let content = Content {
            id: content_id,
            audio: None,
            takes: Vec::new(),
            notes,
            expressions: Vec::new(),
        };
        let clip = Placement {
            id: clip_id,
            content: content_id,
            lane: Lane::Timeline,
            start: Tick::new(0),
            length: Tick::new(window + 1),
            offset: Tick::new(0),
            kind: ClipKind::Fixed,
            gain: 1.0,
            fade_in: DEFAULT_FADE_FLICKS,
            fade_out: DEFAULT_FADE_FLICKS,
        };
        let track = Track {
            id: track_id,
            name: "sparse".into(),
            parent: None,
            order: initial_order(),
            clips: vec![clip],
            devices: vec![test_mixer(12)],
        };
        let project = Project::try_assemble(
            Meter::four_four(),
            TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
            vec![content],
            vec![track],
        )
        .unwrap();
        let index = ArrangeIndex::build(&project);
        let content = project.content(content_id).unwrap();
        let (hits, visits) = index.note_visits(content, window, window + 1);
        assert_eq!(hits, LONG as u32);
        assert!(visits < (NOTES as u64) / 8);
    }
}
