//! Sorted lists kept as shared runs (FCA-19, ADR 0287, ADR 0353).
//!
//! Undo keeps the version before each edit. With one vector per list, an edit
//! of one item copied every item of the list into that version. Runs are
//! shared with `Arc`, so an edit copies only the runs it touches and the
//! version before it keeps the rest in common with the version after it.
//!
//! Where a run ends depends only on the items, never on the edits that led to
//! them: a run ends after an item whose id hashes to a boundary, and at the
//! end of the list. Equal lists therefore have equal runs, and moving one item
//! changes only the run it leaves and the run it enters. About one item in
//! [`MEAN_RUN`] is a boundary. A list whose ids never hash to a boundary is
//! one run, which costs what the single vector did.
//!
//! The notes of a content and the placements of a track use this one rule.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::{Index, Range};
use std::sync::Arc;

use crate::id::Id;
use crate::model::{Note, Placement};

/// Expected items per run. A power of two, so the boundary test is a mask.
pub const MEAN_RUN: usize = 64;

/// An item of a [`Runs`] list: its id places the run boundaries, and the
/// list keeps the items in `run_order`.
pub trait RunItem: Clone + PartialEq {
    fn run_id(&self) -> Id;
    fn run_order(&self, other: &Self) -> Ordering;
}

impl RunItem for Note {
    fn run_id(&self) -> Id {
        self.id
    }

    fn run_order(&self, other: &Self) -> Ordering {
        crate::model::note_ord(self, other)
    }
}

impl RunItem for Placement {
    fn run_id(&self) -> Id {
        self.id
    }

    fn run_order(&self, other: &Self) -> Ordering {
        crate::model::clip_ord(self, other)
    }
}

/// Sorted notes of one content (ADR 0287).
pub type Notes = Runs<Note>;

/// Placements of one track in timeline order (ADR 0353).
pub type Placements = Runs<Placement>;

/// A sorted list stored as shared runs.
///
/// Every run is non-empty. `ends[i]` is the number of items in `runs[..=i]`.
pub struct Runs<T> {
    runs: Vec<Arc<[T]>>,
    ends: Vec<usize>,
}

impl<T> Clone for Runs<T> {
    fn clone(&self) -> Self {
        Self {
            runs: self.runs.clone(),
            ends: self.ends.clone(),
        }
    }
}

impl<T> Default for Runs<T> {
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            ends: Vec::new(),
        }
    }
}

impl<T: RunItem> Runs<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ends.last().copied().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        let run = self.run_of(index);
        self.runs.get(run)?.get(index - self.run_start(run))
    }

    pub fn first(&self) -> Option<&T> {
        self.runs.first().and_then(|run| run.first())
    }

    pub fn last(&self) -> Option<&T> {
        self.runs.last().and_then(|run| run.last())
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            runs: self.runs.iter(),
            front: [].iter(),
            back: [].iter(),
            remaining: self.len(),
        }
    }

    /// Reads by index without a search while the indices stay in one run.
    pub fn cursor(&self) -> Cursor<'_, T> {
        Cursor {
            list: self,
            run: self.runs.first().map_or(&[], |run| &run[..]),
            start: 0,
        }
    }

    /// The shared runs, in list order.
    pub fn runs(&self) -> &[Arc<[T]>] {
        &self.runs
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }

    /// Like [`slice::partition_point`]: the list must be partitioned by `pred`.
    pub fn partition_point(&self, mut pred: impl FnMut(&T) -> bool) -> usize {
        let run = self
            .runs
            .partition_point(|run| run.last().is_some_and(&mut pred));
        match self.runs.get(run) {
            Some(items) => self.run_start(run) + items.partition_point(pred),
            None => self.len(),
        }
    }

    /// Replaces the items in `range` with `insert` and returns the removed
    /// items, like [`Vec::splice`].
    ///
    /// Only the runs that hold `range` are rebuilt, plus the next run when the
    /// rebuilt items no longer end at a boundary. A rebuilt run that equals an
    /// old one keeps the old `Arc`. A range past the end is clamped to it.
    pub fn splice(&mut self, range: Range<usize>, insert: impl IntoIterator<Item = T>) -> Vec<T> {
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
        let mut window: Vec<T> = self.runs[first..after]
            .iter()
            .flat_map(|run| run.iter().cloned())
            .collect();
        let removed: Vec<T> = window
            .splice(start - window_start..end - window_start, insert)
            .collect();
        // The run before `first` ends at a boundary, so the window starts one.
        // Its end must be one too, or the next run belongs to the same run.
        while after < self.runs.len() && window.last().is_some_and(|item| !ends_run(item.run_id()))
        {
            window.extend(self.runs[after].iter().cloned());
            after += 1;
        }
        let rebuilt = chunk_reusing(window, &self.runs[first..after]);
        self.runs.splice(first..after, rebuilt);
        self.recount_from(first);
        removed
    }

    /// Writes one item through `change` and copies only the runs it touches.
    ///
    /// The caller keeps the order: a change of the fields `run_order` reads
    /// must leave the item where that order puts it.
    pub fn update<R>(&mut self, index: usize, change: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut item = self.get(index)?.clone();
        let result = change(&mut item);
        self.splice(index..index + 1, [item]);
        Some(result)
    }

    pub fn remove(&mut self, index: usize) -> Option<T> {
        self.get(index)?;
        self.splice(index..index + 1, []).pop()
    }

    /// Replaces the item at `index` with `item` and moves it to its sorted place.
    ///
    /// An item that stays between its neighbors is written in place. Returns
    /// the new index, or `None` when `index` is past the end.
    pub fn replace_sorted(&mut self, index: usize, item: T) -> Option<usize> {
        self.get(index)?;
        let after_previous = index
            .checked_sub(1)
            .and_then(|previous| self.get(previous))
            .is_none_or(|previous| previous.run_order(&item).is_le());
        let before_next = self
            .get(index + 1)
            .is_none_or(|next| item.run_order(next).is_le());
        if after_previous && before_next {
            self.splice(index..index + 1, [item]);
            return Some(index);
        }
        self.splice(index..index + 1, []);
        Some(self.insert_sorted(item))
    }

    /// Inserts `item` at its sorted place and returns that index.
    pub fn insert_sorted(&mut self, item: T) -> usize {
        let index = self.partition_point(|other| other.run_order(&item).is_lt());
        self.splice(index..index, [item]);
        index
    }

    /// Appends `item`, like [`Vec::push`]. The caller keeps the order.
    pub fn push(&mut self, item: T) {
        let end = self.len();
        self.splice(end..end, [item]);
    }

    /// Keeps the items `keep` accepts, like [`Vec::retain`].
    pub fn retain(&mut self, keep: impl FnMut(&T) -> bool) {
        self.edit(|items| items.retain(keep));
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Edits the whole list as a vector, then shares the runs that did not change.
    ///
    /// This copies every item once. Use it for edits that move many items.
    /// The caller leaves the vector in `run_order`.
    pub fn edit<R>(&mut self, edit: impl FnOnce(&mut Vec<T>) -> R) -> R {
        let mut items = self.to_vec();
        let result = edit(&mut items);
        let runs = chunk_reusing(items, &self.runs);
        self.runs = runs;
        self.recount_from(0);
        result
    }

    fn from_vec(items: Vec<T>) -> Self {
        let mut list = Self {
            runs: chunk_reusing(items, &[]),
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

/// Whether a run ends after the item with `id`. The id bits are mixed first:
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

/// Splits `items` into canonical runs, keeping an `old` run wherever an equal
/// run comes out. Old runs are matched by their first item's id.
fn chunk_reusing<T: RunItem>(items: Vec<T>, old: &[Arc<[T]>]) -> Vec<Arc<[T]>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, item) in items.iter().enumerate() {
        if ends_run(item.run_id()) {
            ranges.push(start..index + 1);
            start = index + 1;
        }
    }
    if start < items.len() {
        ranges.push(start..items.len());
    }
    let by_first: HashMap<Id, &Arc<[T]>> = old
        .iter()
        .filter_map(|run| run.first().map(|item| (item.run_id(), run)))
        .collect();
    let kept: Vec<Option<Arc<[T]>>> = ranges
        .iter()
        .map(|range| {
            by_first
                .get(&items[range.start].run_id())
                .filter(|run| run[..] == items[range.clone()])
                .map(|run| Arc::clone(run))
        })
        .collect();
    let mut rest = items.into_iter();
    let mut runs = Vec::with_capacity(ranges.len());
    for (range, kept) in ranges.iter().zip(kept) {
        let items = rest.by_ref().take(range.len());
        match kept {
            Some(run) => {
                items.for_each(drop);
                runs.push(run);
            }
            None => runs.push(items.collect()),
        }
    }
    runs
}

impl<T: RunItem> From<Vec<T>> for Runs<T> {
    fn from(items: Vec<T>) -> Self {
        Self::from_vec(items)
    }
}

impl<T: RunItem> FromIterator<T> for Runs<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from_vec(iter.into_iter().collect())
    }
}

impl<T: RunItem> PartialEq for Runs<T> {
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

impl<T: RunItem + Eq> Eq for Runs<T> {}

impl<T: RunItem + fmt::Debug> fmt::Debug for Runs<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: RunItem> Index<usize> for Runs<T> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        match self.get(index) {
            Some(item) => item,
            None => panic!(
                "run item index {index} is out of range for {} items",
                self.len()
            ),
        }
    }
}

impl<'a, T: RunItem> IntoIterator for &'a Runs<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

/// Reads items by index; [`Runs::get`] searches the runs on every call.
///
/// Indices that mostly increase, as an in-order tree walk produces, find the
/// item in the run of the last read.
pub struct Cursor<'a, T> {
    list: &'a Runs<T>,
    /// The run of the last read, which starts at list index `start`.
    run: &'a [T],
    start: usize,
}

impl<'a, T: RunItem> Cursor<'a, T> {
    pub fn get(&mut self, index: usize) -> Option<&'a T> {
        // An index before `start` wraps past the run and takes the search.
        if let Some(item) = self.run.get(index.wrapping_sub(self.start)) {
            return Some(item);
        }
        let run = self.list.run_of(index);
        self.start = self.list.run_start(run);
        self.run = self.list.runs.get(run).map_or(&[], |run| &run[..]);
        self.run.get(index - self.start)
    }
}

/// Iterates the items of a [`Runs`] list in order.
pub struct Iter<'a, T> {
    runs: std::slice::Iter<'a, Arc<[T]>>,
    front: std::slice::Iter<'a, T>,
    back: std::slice::Iter<'a, T>,
    remaining: usize,
}

impl<T> Clone for Iter<'_, T> {
    fn clone(&self) -> Self {
        Self {
            runs: self.runs.clone(),
            front: self.front.clone(),
            back: self.back.clone(),
            remaining: self.remaining,
        }
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        loop {
            if let Some(item) = self.front.next() {
                self.remaining -= 1;
                return Some(item);
            }
            match self.runs.next() {
                Some(run) => self.front = run.iter(),
                None => {
                    let item = self.back.next()?;
                    self.remaining -= 1;
                    return Some(item);
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T> DoubleEndedIterator for Iter<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.back.next_back() {
                self.remaining -= 1;
                return Some(item);
            }
            match self.runs.next_back() {
                Some(run) => self.back = run.iter(),
                None => {
                    let item = self.front.next_back()?;
                    self.remaining -= 1;
                    return Some(item);
                }
            }
        }
    }
}

impl<T> ExactSizeIterator for Iter<'_, T> {}

impl<T> FusedIterator for Iter<'_, T> {}

#[cfg(test)]
#[path = "runs_tests.rs"]
mod tests;
