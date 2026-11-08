//! The notes of one content, kept as shared runs (FCA-19, ADR 0287).
//!
//! Undo keeps the version before each edit. With one vector per content, a
//! note edit copied every note of the content into that version. Runs are
//! shared with `Arc`, so an edit copies only the runs it touches and the
//! version before it keeps the rest in common with the version after it.
//!
//! Where a run ends depends only on the notes, never on the edits that led to
//! them: a run ends after a note whose id hashes to a boundary, and at the end
//! of the list. Equal lists therefore have equal runs, and moving one note
//! changes only the run it leaves and the run it enters. About one note in
//! [`MEAN_RUN`] is a boundary. A list whose ids never hash to a boundary is
//! one run, which costs what the single vector did.

use std::collections::HashMap;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::{Index, Range};
use std::sync::Arc;

use crate::id::Id;
use crate::model::Note;

/// Expected notes per run. A power of two, so the boundary test is a mask.
pub const MEAN_RUN: usize = 64;

/// Sorted notes of one content, stored as shared runs.
///
/// Every run is non-empty. `ends[i]` is the number of notes in `runs[..=i]`.
#[derive(Clone, Default)]
pub struct Notes {
    runs: Vec<Arc<[Note]>>,
    ends: Vec<usize>,
}

impl Notes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ends.last().copied().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Note> {
        let run = self.run_of(index);
        self.runs.get(run)?.get(index - self.run_start(run))
    }

    pub fn first(&self) -> Option<&Note> {
        self.runs.first().and_then(|run| run.first())
    }

    pub fn last(&self) -> Option<&Note> {
        self.runs.last().and_then(|run| run.last())
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            runs: self.runs.iter(),
            front: [].iter(),
            back: [].iter(),
            remaining: self.len(),
        }
    }

    /// Reads by index without a search while the indices stay in one run.
    pub fn cursor(&self) -> Cursor<'_> {
        Cursor {
            notes: self,
            run: self.runs.first().map_or(&[], |run| &run[..]),
            start: 0,
        }
    }

    /// The shared runs, in list order.
    pub fn runs(&self) -> &[Arc<[Note]>] {
        &self.runs
    }

    pub fn to_vec(&self) -> Vec<Note> {
        self.iter().cloned().collect()
    }

    /// Like [`slice::partition_point`]: the list must be partitioned by `pred`.
    pub fn partition_point(&self, mut pred: impl FnMut(&Note) -> bool) -> usize {
        let run = self
            .runs
            .partition_point(|run| run.last().is_some_and(&mut pred));
        match self.runs.get(run) {
            Some(notes) => self.run_start(run) + notes.partition_point(pred),
            None => self.len(),
        }
    }

    /// Replaces the notes in `range` with `insert` and returns the removed
    /// notes, like [`Vec::splice`].
    ///
    /// Only the runs that hold `range` are rebuilt, plus the next run when the
    /// rebuilt notes no longer end at a boundary. A rebuilt run that equals an
    /// old one keeps the old `Arc`. A range past the end is clamped to it.
    pub fn splice(
        &mut self,
        range: Range<usize>,
        insert: impl IntoIterator<Item = Note>,
    ) -> Vec<Note> {
        let len = self.len();
        let end = range.end.min(len);
        let start = range.start.min(end);
        debug_assert!(range.start <= range.end && range.end <= len);
        if self.runs.is_empty() {
            *self = Self::from_vec(insert.into_iter().collect());
            return Vec::new();
        }
        let last_run = self.runs.len() - 1;
        let first = self.run_of(start).min(last_run);
        let mut after = if end > start {
            self.run_of(end - 1).min(last_run)
        } else {
            first
        } + 1;
        let window_start = self.run_start(first);
        let mut window: Vec<Note> = self.runs[first..after]
            .iter()
            .flat_map(|run| run.iter().cloned())
            .collect();
        let removed: Vec<Note> = window
            .splice(start - window_start..end - window_start, insert)
            .collect();
        // The run before `first` ends at a boundary, so the window starts one.
        // Its end must be one too, or the next run belongs to the same run.
        while after < self.runs.len() && window.last().is_some_and(|note| !ends_run(note.id)) {
            window.extend(self.runs[after].iter().cloned());
            after += 1;
        }
        let rebuilt = chunk_reusing(window, &self.runs[first..after]);
        self.runs.splice(first..after, rebuilt);
        self.recount_from(first);
        removed
    }

    /// Writes one note through `change` and copies only the runs it touches.
    ///
    /// The caller keeps the order: a change of start, pitch, or id must
    /// leave the note where [`crate::model::note_ord`] puts it.
    pub fn update<R>(&mut self, index: usize, change: impl FnOnce(&mut Note) -> R) -> Option<R> {
        let mut note = self.get(index)?.clone();
        let result = change(&mut note);
        self.splice(index..index + 1, [note]);
        Some(result)
    }

    pub fn remove(&mut self, index: usize) -> Option<Note> {
        self.get(index)?;
        self.splice(index..index + 1, []).pop()
    }

    /// Replaces the note at `index` with `note` and moves it to its sorted place.
    ///
    /// A note that stays between its neighbors is written in place. Returns
    /// the new index, or `None` when `index` is past the end.
    pub fn replace_sorted(&mut self, index: usize, note: Note) -> Option<usize> {
        self.get(index)?;
        let after_previous = index
            .checked_sub(1)
            .and_then(|previous| self.get(previous))
            .is_none_or(|previous| crate::model::note_ord(previous, &note).is_le());
        let before_next = self
            .get(index + 1)
            .is_none_or(|next| crate::model::note_ord(&note, next).is_le());
        if after_previous && before_next {
            self.splice(index..index + 1, [note]);
            return Some(index);
        }
        self.splice(index..index + 1, []);
        Some(self.insert_sorted(note))
    }

    /// Inserts `note` at its sorted place and returns that index.
    pub fn insert_sorted(&mut self, note: Note) -> usize {
        let index = self.partition_point(|other| crate::model::note_ord(other, &note).is_lt());
        self.splice(index..index, [note]);
        index
    }

    /// Edits the whole list as a vector, then shares the runs that did not change.
    ///
    /// This copies every note once. Use it for edits that move many notes.
    pub fn edit<R>(&mut self, edit: impl FnOnce(&mut Vec<Note>) -> R) -> R {
        let mut notes = self.to_vec();
        let result = edit(&mut notes);
        let runs = chunk_reusing(notes, &self.runs);
        self.runs = runs;
        self.recount_from(0);
        result
    }

    fn from_vec(notes: Vec<Note>) -> Self {
        let mut list = Self {
            runs: chunk_reusing(notes, &[]),
            ends: Vec::new(),
        };
        list.recount_from(0);
        list
    }

    /// The run holding `index`, or `runs.len()` past the end.
    fn run_of(&self, index: usize) -> usize {
        self.ends.partition_point(|&end| end <= index)
    }

    fn run_start(&self, run: usize) -> usize {
        run.checked_sub(1)
            .and_then(|previous| self.ends.get(previous))
            .copied()
            .unwrap_or(0)
    }

    fn recount_from(&mut self, first: usize) {
        self.ends.truncate(first);
        let mut total = self.ends.last().copied().unwrap_or(0);
        for run in &self.runs[first..] {
            total += run.len();
            self.ends.push(total);
        }
    }
}

/// Whether a run ends after the note with `id`. The id bits are mixed first:
/// generated ids carry a clock, and test ids count up from one.
pub(crate) fn ends_run(id: Id) -> bool {
    let raw = id.raw();
    let mut bits = (raw as u64) ^ ((raw >> 64) as u64);
    // SplitMix64's finalizer.
    bits = (bits ^ (bits >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    bits = (bits ^ (bits >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    bits ^= bits >> 31;
    bits & (MEAN_RUN as u64 - 1) == 0
}

/// Splits `notes` into canonical runs, keeping an `old` run wherever an equal
/// run comes out. Old runs are matched by their first note's id.
fn chunk_reusing(notes: Vec<Note>, old: &[Arc<[Note]>]) -> Vec<Arc<[Note]>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, note) in notes.iter().enumerate() {
        if ends_run(note.id) {
            ranges.push(start..index + 1);
            start = index + 1;
        }
    }
    if start < notes.len() {
        ranges.push(start..notes.len());
    }
    let by_first: HashMap<Id, &Arc<[Note]>> = old
        .iter()
        .filter_map(|run| run.first().map(|note| (note.id, run)))
        .collect();
    let kept: Vec<Option<Arc<[Note]>>> = ranges
        .iter()
        .map(|range| {
            by_first
                .get(&notes[range.start].id)
                .filter(|run| run[..] == notes[range.clone()])
                .map(|run| Arc::clone(run))
        })
        .collect();
    let mut items = notes.into_iter();
    let mut runs = Vec::with_capacity(ranges.len());
    for (range, kept) in ranges.iter().zip(kept) {
        let notes = items.by_ref().take(range.len());
        match kept {
            Some(run) => {
                notes.for_each(drop);
                runs.push(run);
            }
            None => runs.push(notes.collect()),
        }
    }
    runs
}

impl From<Vec<Note>> for Notes {
    fn from(notes: Vec<Note>) -> Self {
        Self::from_vec(notes)
    }
}

impl FromIterator<Note> for Notes {
    fn from_iter<T: IntoIterator<Item = Note>>(iter: T) -> Self {
        Self::from_vec(iter.into_iter().collect())
    }
}

impl PartialEq for Notes {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        // Equal lists have equal runs, so shared runs settle most comparisons.
        let same_runs = self.runs.len() == other.runs.len()
            && self
                .runs
                .iter()
                .zip(&other.runs)
                .all(|(left, right)| Arc::ptr_eq(left, right) || left == right);
        same_runs || self.iter().eq(other.iter())
    }
}

impl Eq for Notes {}

impl fmt::Debug for Notes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl Index<usize> for Notes {
    type Output = Note;

    fn index(&self, index: usize) -> &Note {
        match self.get(index) {
            Some(note) => note,
            None => panic!(
                "note index {index} is out of range for {} notes",
                self.len()
            ),
        }
    }
}

impl<'a> IntoIterator for &'a Notes {
    type Item = &'a Note;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

/// Reads notes by index; [`Notes::get`] searches the runs on every call.
///
/// Indices that mostly increase, as an in-order tree walk produces, find the
/// note in the run of the last read.
pub struct Cursor<'a> {
    notes: &'a Notes,
    /// The run of the last read, which starts at list index `start`.
    run: &'a [Note],
    start: usize,
}

impl<'a> Cursor<'a> {
    pub fn get(&mut self, index: usize) -> Option<&'a Note> {
        // An index before `start` wraps past the run and takes the search.
        if let Some(note) = self.run.get(index.wrapping_sub(self.start)) {
            return Some(note);
        }
        let run = self.notes.run_of(index);
        self.start = self.notes.run_start(run);
        self.run = self.notes.runs.get(run).map_or(&[], |run| &run[..]);
        self.run.get(index - self.start)
    }
}

/// Iterates the notes of a [`Notes`] in order.
#[derive(Clone)]
pub struct Iter<'a> {
    runs: std::slice::Iter<'a, Arc<[Note]>>,
    front: std::slice::Iter<'a, Note>,
    back: std::slice::Iter<'a, Note>,
    remaining: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Note;

    fn next(&mut self) -> Option<&'a Note> {
        loop {
            if let Some(note) = self.front.next() {
                self.remaining -= 1;
                return Some(note);
            }
            match self.runs.next() {
                Some(run) => self.front = run.iter(),
                None => {
                    let note = self.back.next()?;
                    self.remaining -= 1;
                    return Some(note);
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(note) = self.back.next_back() {
                self.remaining -= 1;
                return Some(note);
            }
            match self.runs.next_back() {
                Some(run) => self.back = run.iter(),
                None => {
                    let note = self.front.next_back()?;
                    self.remaining -= 1;
                    return Some(note);
                }
            }
        }
    }
}

impl ExactSizeIterator for Iter<'_> {}

impl FusedIterator for Iter<'_> {}

#[cfg(test)]
#[path = "notes_tests.rs"]
mod tests;
