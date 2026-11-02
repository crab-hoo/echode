//! One published version of the project, as the app reads it.

use std::collections::BTreeSet;
use std::sync::Arc;

use echode_audio_io::AudioState;
use echode_document::{EMPTY_DEVICE_REPORT, EMPTY_HISTORY_REPORT, Presenter, Publication};
use echode_draw::{ArrangeIndex, ArrangeView, Element, ElementId, PianoNote, PianoView, Role};
use echode_history::{PeakColumn, Waveforms};
use echode_model::{Id, Project};

use crate::{PianoNoteElement, TrackTreeInfo, VisibleElement};

/// One owned reference to a project snapshot and the arrange index for that version.
pub struct Snapshot {
    project: Arc<Project>,
    pub(crate) controls: Arc<crate::controls::ControlIndex>,
    index: Arc<ArrangeIndex>,
    audio: Arc<Vec<u8>>,
    history: Arc<Vec<u8>>,
    device: Arc<Vec<u8>>,
    waveforms: Arc<Waveforms>,
    hierarchy: Arc<Vec<TrackTreeInfo>>,
    row_map: Option<Arc<Vec<usize>>>,
}

/// Builds each turn's snapshot on the document worker. An unchanged project
/// reuses the previous arrange index and waveforms.
#[derive(Default)]
pub(crate) struct SnapshotPresenter {
    previous: Option<Snapshot>,
}

impl Presenter for SnapshotPresenter {
    type View = Snapshot;

    fn present(&mut self, publication: Publication<'_>) -> Snapshot {
        let Publication {
            project,
            directory,
            audio,
            history,
            device,
        } = publication;
        let previous = self.previous.as_ref();
        let mut snapshot = if let Some(previous) = previous
            && Arc::ptr_eq(&project, &previous.project)
        {
            previous.retain()
        } else {
            let mut snapshot = Snapshot::from_project(
                Arc::clone(&project),
                previous.map(|snapshot| &*snapshot.index),
            );
            snapshot.waveforms = Arc::new(Waveforms::load(
                directory,
                &project,
                previous.map(|snapshot| snapshot.waveforms.as_ref()),
            ));
            snapshot
        };
        snapshot.audio = Arc::new(audio);
        snapshot.history = Arc::new(history);
        snapshot.device = Arc::new(device);
        self.previous = Some(snapshot.retain());
        snapshot
    }

    fn with_device_report(view: Snapshot, report: Vec<u8>) -> Snapshot {
        view.with_device(report)
    }
}

impl Snapshot {
    pub(crate) fn flicks_at(&self, tick: i64) -> Option<i64> {
        self.project
            .tempo()
            .flicks_at(echode_model::Tick::new(tick))
            .map(|time| time.get())
    }
    pub(crate) fn from_project(project: Arc<Project>, previous: Option<&ArrangeIndex>) -> Self {
        let index = previous.map_or_else(
            || ArrangeIndex::build(&project),
            |index| index.update(&project),
        );
        Self {
            controls: Arc::new(crate::controls::ControlIndex::build(&project)),
            hierarchy: Arc::new(crate::projection::hierarchy(&project)),
            row_map: None,
            project,
            index: Arc::new(index),
            audio: Arc::new(serde_json::to_vec(&AudioState::default()).unwrap_or_default()),
            history: Arc::new(EMPTY_HISTORY_REPORT.to_vec()),
            device: Arc::new(EMPTY_DEVICE_REPORT.to_vec()),
            waveforms: Arc::new(Waveforms::empty()),
        }
    }

    pub(crate) fn retain(&self) -> Self {
        Self {
            project: Arc::clone(&self.project),
            controls: Arc::clone(&self.controls),
            index: Arc::clone(&self.index),
            audio: Arc::clone(&self.audio),
            history: Arc::clone(&self.history),
            device: Arc::clone(&self.device),
            waveforms: Arc::clone(&self.waveforms),
            hierarchy: Arc::clone(&self.hierarchy),
            row_map: self.row_map.clone(),
        }
    }

    pub(crate) fn project_arrange(&self, collapsed: &BTreeSet<Id>) -> Self {
        if collapsed.is_empty() {
            let mut projected = self.retain();
            projected.row_map = None;
            return projected;
        }
        let mut hidden = BTreeSet::new();
        let rows =
            self.project
                .tracks()
                .iter()
                .enumerate()
                .filter_map(|(index, track)| {
                    if track.parent.is_some_and(|parent| {
                        collapsed.contains(&parent) || hidden.contains(&parent)
                    }) {
                        hidden.insert(track.id);
                        None
                    } else {
                        Some(index)
                    }
                })
                .collect();
        let mut projected = self.retain();
        projected.row_map = Some(Arc::new(rows));
        projected
    }

    pub(crate) fn source_track_index(&self, row: usize) -> Option<usize> {
        match &self.row_map {
            Some(rows) => rows.get(row).copied(),
            None => (row < self.project.tracks().len()).then_some(row),
        }
    }

    pub(crate) fn track_tree(&self, row: usize) -> Option<TrackTreeInfo> {
        self.hierarchy.get(self.source_track_index(row)?).copied()
    }

    fn track(&self, row: usize) -> Option<&echode_model::Track> {
        self.project
            .tracks()
            .get(self.source_track_index(row)?)
            .map(AsRef::as_ref)
    }

    pub(crate) fn waveform(
        &self,
        placement: Id,
        start_tick: i64,
        end_tick: i64,
        columns: &mut [PeakColumn],
    ) -> usize {
        self.waveforms
            .fill(&self.project, placement, start_tick, end_tick, columns)
    }

    pub(crate) fn audio(&self) -> &[u8] {
        &self.audio
    }

    pub(crate) fn history(&self) -> &[u8] {
        &self.history
    }

    pub(crate) fn device(&self) -> &[u8] {
        &self.device
    }

    /// The same snapshot, reporting a device command's result.
    fn with_device(mut self, report: Vec<u8>) -> Self {
        self.device = Arc::new(report);
        self
    }

    pub(crate) fn track_count(&self) -> usize {
        self.row_map
            .as_ref()
            .map_or(self.project.tracks().len(), |rows| rows.len())
    }

    pub(crate) fn write_track_id(&self, index: usize, out: &mut [u8; 16]) -> bool {
        let Some(track) = self.track(index) else {
            return false;
        };
        *out = id_bytes(track.id);
        true
    }

    pub(crate) fn device_count(&self, track: usize) -> usize {
        self.track(track)
            .map(|track| track.devices.len())
            .unwrap_or(0)
    }

    pub(crate) fn write_device_id(&self, track: usize, device: usize, out: &mut [u8; 16]) -> bool {
        let Some(device) = self
            .track(track)
            .and_then(|track| track.devices.get(device))
        else {
            return false;
        };
        *out = id_bytes(device.id);
        true
    }

    pub(crate) fn write_visible(
        &self,
        view: ArrangeView,
        out: &mut [VisibleElement],
    ) -> (usize, usize) {
        let mut written = 0usize;
        let mut dropped = 0usize;
        let emit = |element| {
            if written < out.len() {
                out[written] = visible_element(element);
                written += 1;
            } else {
                dropped = dropped.saturating_add(1);
            }
        };
        if let Some(rows) = &self.row_map {
            self.index
                .visit_visible_rows(&self.project, view, rows, emit);
        } else {
            self.index.visit_visible(&self.project, view, emit);
        }
        (written, dropped)
    }

    pub(crate) fn write_piano(
        &self,
        track: Id,
        view: PianoView,
        out: &mut [PianoNoteElement],
    ) -> (usize, usize) {
        let mut written = 0;
        let mut dropped = 0usize;
        self.index.visit_piano(&self.project, track, view, |note| {
            if written < out.len() {
                out[written] = piano_element(note);
                written += 1;
            } else {
                dropped = dropped.saturating_add(1);
            }
        });
        (written, dropped)
    }

    pub(crate) fn piano_note(
        &self,
        track: Id,
        placement: Option<Id>,
        note: Id,
    ) -> Option<PianoNoteElement> {
        self.index
            .piano_note(&self.project, track, placement, note)
            .map(piano_element)
    }

    pub(crate) fn piano_neighbor(
        &self,
        track: Id,
        placement: Option<Id>,
        note: Option<Id>,
        backwards: bool,
    ) -> Option<PianoNoteElement> {
        let current = note.map(|note| ElementId {
            role: Role::Note,
            track,
            placement,
            note: Some(note),
        });
        self.index
            .piano_neighbor(&self.project, track, current, backwards)
            .map(piano_element)
    }
}

// Generate the mechanical scalar queries, not pointer handling or C layout.
macro_rules! device_accessor {
    (skip, $getter:ident, $field:ident, $type:ty) => {};
    (copy, $getter:ident, $field:ident, $type:ty) => {
        pub(crate) fn $getter(&self, track: usize, device: usize) -> Option<$type> {
            Some(self.track(track)?.devices.get(device)?.$field)
        }
    };
    (text, $getter:ident, $field:ident, $type:ty) => {
        pub(crate) fn $getter(&self, track: usize, device: usize) -> Option<&str> {
            self.track(track)?.devices.get(device)?.$field.as_deref()
        }
    };
    (slice, $getter:ident, $field:ident, $type:ty) => {
        pub(crate) fn $getter(&self, track: usize, device: usize) -> Option<&[u32]> {
            Some(self.track(track)?.devices.get(device)?.$field.as_slice())
        }
    };
}
macro_rules! snapshot_device_fields {
    (
        fields { $( $(#[$meta:meta])* $property:ident => $field:ident: $type:ty,
            $encode:ident, $read:ident, $parse:ident; $ffi:ident $getter:ident;)+ }
        create [$($create:ident),+]; diff [$($diff:ident),+];
    ) => {
        impl Snapshot { $(device_accessor!($ffi, $getter, $field, $type);)+ }
    };
}
echode_model::device_fields!(snapshot_device_fields);

macro_rules! track_accessor {
    (skip, $getter:ident, $field:ident) => {};
    (text, $getter:ident, $field:ident) => {
        pub(crate) fn $getter(&self, row: usize) -> Option<&str> {
            Some(self.track(row)?.$field.as_str())
        }
    };
}
macro_rules! snapshot_track_fields {
    ($($(#[$attribute:meta])* $property:ident => $field:ident: $type:ty,
        $value:ident, $read:ident; $ffi:ident $getter:ident;)+) => {
        impl Snapshot { $(track_accessor!($ffi, $getter, $field);)+ }
    };
}
echode_model::track_fields!(snapshot_track_fields);

fn piano_element(note: PianoNote) -> PianoNoteElement {
    PianoNoteElement {
        track_id: id_bytes(note.id.track),
        placement_id: note.id.placement.map(id_bytes).unwrap_or([0; 16]),
        note_id: note.id.note.map(id_bytes).unwrap_or([0; 16]),
        start: note.start.get(),
        length: note.length.get(),
        window_start: note.window_start.get(),
        window_end: note.window_end.get(),
        pitch: u32::from(note.pitch),
        velocity: u32::from(note.velocity),
        x: note.x,
        y: note.y,
        w: note.w,
        h: note.h,
    }
}

fn visible_element(element: Element) -> VisibleElement {
    let role = match element.id.role {
        Role::Track => 0,
        Role::Clip => 1,
        Role::Note => 2,
    };
    VisibleElement {
        role,
        track_id: id_bytes(element.id.track),
        placement_id: element.id.placement.map(id_bytes).unwrap_or([0; 16]),
        note_id: element.id.note.map(id_bytes).unwrap_or([0; 16]),
        has_placement: u8::from(element.id.placement.is_some()),
        has_note: u8::from(element.id.note.is_some()),
        pad: [0; 2],
        x: element.x,
        y: element.y,
        w: element.w,
        h: element.h,
    }
}

fn id_bytes(id: Id) -> [u8; 16] {
    *uuid::Uuid::from_u128(id.raw()).as_bytes()
}
