use std::sync::Arc;

use super::{MEAN_RUN, Notes, ends_run};
use crate::model::{Note, note_ord, sort_notes};
use crate::{Id, Tick};

fn note(id: u128, start: i64, pitch: u8) -> Note {
    Note {
        id: Id::from_raw(id),
        start: Tick::new(start),
        length: Tick::new(240),
        pitch,
        velocity: 100,
        release: 0,
        articulation: None,
    }
}

fn sorted(count: u128) -> Vec<Note> {
    let mut notes: Vec<Note> = (1..=count)
        .map(|id| note(id, (id as i64 % 97) * 120, (id % 128) as u8))
        .collect();
    sort_notes(&mut notes);
    notes
}

/// Every run is non-empty and ends exactly at its first boundary note.
fn assert_canonical(notes: &Notes) {
    let runs = notes.runs();
    for (index, run) in runs.iter().enumerate() {
        assert!(!run.is_empty(), "run {index} is empty");
        let (last, inner) = run.split_last().unwrap();
        assert!(
            inner.iter().all(|note| !ends_run(note.id)),
            "run {index} holds a boundary before its end"
        );
        assert!(
            index + 1 == runs.len() || ends_run(last.id),
            "run {index} ends without a boundary"
        );
    }
    let mut total = 0;
    for (run, end) in runs.iter().zip(&notes.ends) {
        total += run.len();
        assert_eq!(*end, total);
    }
    assert_eq!(notes.ends.len(), runs.len());
}

fn unshared(after: &Notes, before: &Notes) -> usize {
    after
        .runs()
        .iter()
        .filter(|run| !before.runs().iter().any(|old| Arc::ptr_eq(run, old)))
        .map(|run| run.len())
        .sum()
}

/// Deterministic xorshift, so a failure names its step.
struct Steps(u64);

impl Steps {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound.max(1) as u64) as usize
    }
}

#[test]
fn runs_split_at_boundaries_and_read_like_the_vector() {
    for count in [0, 1, 2, 63, 64, 65, 1_000, 10_000] {
        let vector = sorted(count);
        let notes = Notes::from(vector.clone());
        assert_canonical(&notes);
        assert_eq!(notes.len(), vector.len());
        assert_eq!(notes.is_empty(), vector.is_empty());
        assert_eq!(notes.to_vec(), vector);
        assert!(notes.iter().rev().eq(vector.iter().rev()));
        assert_eq!(notes.iter().len(), vector.len());
        assert_eq!(notes.first(), vector.first());
        assert_eq!(notes.last(), vector.last());
        assert_eq!(format!("{notes:?}"), format!("{vector:?}"));
        for (index, item) in vector.iter().enumerate() {
            assert_eq!(notes.get(index), Some(item));
            assert_eq!(&notes[index], item);
        }
        assert_eq!(notes.get(vector.len()), None);
        for start in [-1, 0, 120, 5_000, 20_000] {
            assert_eq!(
                notes.partition_point(|item| item.start.get() < start),
                vector.partition_point(|item| item.start.get() < start)
            );
        }
        if count >= 1_000 {
            let runs = notes.runs().len();
            let mean = vector.len() / runs;
            assert!(
                (MEAN_RUN / 2..=MEAN_RUN * 2).contains(&mean),
                "{count} notes in {runs} runs"
            );
        }
    }
}

#[test]
fn a_mixed_iterator_meets_in_the_middle() {
    let notes = Notes::from(sorted(1_000));
    let mut iter = notes.iter();
    let mut front = Vec::new();
    let mut back = Vec::new();
    while let Some(item) = iter.next() {
        front.push(item.id);
        assert_eq!(iter.len(), 1_000 - front.len() - back.len());
        if let Some(item) = iter.next_back() {
            back.push(item.id);
        }
    }
    back.reverse();
    front.extend(back);
    let expected: Vec<Id> = notes.to_vec().iter().map(|item| item.id).collect();
    assert_eq!(front, expected);
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);
}

#[test]
fn edits_match_the_vector_keep_canonical_runs_and_share_the_rest() {
    let mut steps = Steps(0x9e37_79b9_7f4a_7c15);
    let mut vector = sorted(2_000);
    let mut notes = Notes::from(vector.clone());
    let mut next_id = 10_000u128;
    for step in 0..2_000 {
        let before = notes.clone();
        let operation = steps.below(6);
        match operation {
            0 => {
                next_id += 1;
                let item = note(next_id, steps.below(12_000) as i64, steps.below(128) as u8);
                let index = notes.insert_sorted(item.clone());
                vector.insert(index, item);
            }
            1 if !vector.is_empty() => {
                let index = steps.below(vector.len());
                assert_eq!(notes.remove(index), Some(vector.remove(index)));
            }
            2 if !vector.is_empty() => {
                let index = steps.below(vector.len());
                let velocity = steps.below(128) as u16;
                notes.update(index, |item| item.velocity = velocity);
                vector[index].velocity = velocity;
            }
            3 if !vector.is_empty() => {
                let index = steps.below(vector.len());
                let mut moved = vector[index].clone();
                moved.start = Tick::new(steps.below(12_000) as i64);
                moved.pitch = steps.below(128) as u8;
                notes.replace_sorted(index, moved.clone());
                vector[index] = moved;
                sort_notes(&mut vector);
            }
            4 => {
                let shift = steps.below(3) as i64;
                let edge = steps.below(12_000) as i64;
                notes.edit(|items| {
                    for item in items.iter_mut().filter(|item| item.start.get() > edge) {
                        item.start = Tick::new(item.start.get() + shift);
                    }
                });
                for item in vector.iter_mut().filter(|item| item.start.get() > edge) {
                    item.start = Tick::new(item.start.get() + shift);
                }
            }
            _ => {
                let start = steps.below(vector.len() + 1);
                let end = (start + steps.below(5)).min(vector.len());
                notes.splice(start..end, []);
                vector.drain(start..end);
            }
        }
        assert_eq!(notes.to_vec(), vector, "step {step} operation {operation}");
        assert!(
            notes
                .iter()
                .zip(notes.iter().skip(1))
                .all(|(a, b)| note_ord(a, b).is_le()),
            "step {step} left the list unsorted"
        );
        assert_canonical(&notes);
        let rebuilt = Notes::from(vector.clone());
        assert_eq!(notes.runs().len(), rebuilt.runs().len(), "step {step}");
        assert_eq!(notes, rebuilt);
        if operation != 4 {
            // A single-note edit rebuilds at most three runs: the one it
            // leaves, the one it enters, and a neighbor joined at a boundary.
            let longest = before.runs().iter().map(|run| run.len()).max().unwrap_or(0);
            assert!(
                unshared(&notes, &before) <= 3 * longest + 1,
                "step {step} operation {operation} copied {} notes",
                unshared(&notes, &before)
            );
        }
    }
}

#[test]
fn one_velocity_edit_keeps_all_but_one_run_of_ten_thousand_notes() {
    let before = Notes::from(sorted(10_000));
    let mut after = before.clone();
    after.update(5_000, |item| item.velocity = 1);
    let changed = after
        .runs()
        .iter()
        .filter(|run| !before.runs().iter().any(|old| Arc::ptr_eq(run, old)));
    assert_eq!(changed.count(), 1);
    assert!(unshared(&after, &before) < 4 * MEAN_RUN);
    assert_eq!(after.runs().len(), before.runs().len());
    assert_ne!(after, before);
}

#[test]
fn an_edit_that_changes_nothing_keeps_every_run() {
    let before = Notes::from(sorted(5_000));
    let mut after = before.clone();
    after.edit(|_| ());
    after.update(17, |_| ());
    for (old, new) in before.runs().iter().zip(after.runs()) {
        assert!(Arc::ptr_eq(old, new));
    }
}

#[test]
fn ids_without_a_boundary_form_one_run() {
    let ids: Vec<u128> = (1..100_000u128)
        .filter(|id| !ends_run(Id::from_raw(*id)))
        .take(300)
        .collect();
    let mut vector: Vec<Note> = ids.iter().map(|id| note(*id, *id as i64, 60)).collect();
    sort_notes(&mut vector);
    let mut notes = Notes::from(vector.clone());
    assert_eq!(notes.runs().len(), 1);
    notes.update(10, |item| item.velocity = 3);
    vector[10].velocity = 3;
    assert_eq!(notes.to_vec(), vector);
    assert_canonical(&notes);
}

#[test]
fn equal_lists_compare_equal_whatever_their_runs_are() {
    let vector = sorted(500);
    let canonical = Notes::from(vector.clone());
    // A list built run by run from the back still has the same notes.
    let mut grown = Notes::new();
    for item in vector.iter().rev() {
        grown.splice(0..0, [item.clone()]);
    }
    assert_canonical(&grown);
    assert_eq!(grown, canonical);
    let mut other = canonical.clone();
    other.update(0, |item| item.pitch = 1);
    assert_ne!(other, canonical);
    assert_ne!(Notes::from(sorted(499)), canonical);
}

#[test]
fn ranges_past_the_end_are_clamped() {
    let mut notes = Notes::from(sorted(10));
    assert_eq!(notes.remove(10), None);
    assert_eq!(notes.update(10, |_| ()), None);
    assert_eq!(notes.replace_sorted(10, note(99, 0, 0)), None);
    assert_eq!(notes.len(), 10);
    let mut empty = Notes::new();
    empty.splice(0..0, [note(1, 0, 60)]);
    assert_eq!(empty.len(), 1);
    empty.splice(0..1, []);
    assert!(empty.is_empty());
    assert!(empty.runs().is_empty());
}

#[test]
fn a_cursor_reads_like_get_in_any_order() {
    let vector = sorted(3_000);
    let notes = Notes::from(vector.clone());
    let mut cursor = notes.cursor();
    let mut steps = Steps(7);
    for index in (0..vector.len()).chain((0..vector.len()).rev()) {
        assert_eq!(cursor.get(index), vector.get(index));
    }
    for _ in 0..5_000 {
        let index = steps.below(vector.len() + 2);
        assert_eq!(cursor.get(index), vector.get(index));
    }
    assert_eq!(Notes::new().cursor().get(0), None);
}
