//! Work bounds for the product changeset path, separate from model editing.
//! Content and note counts constrain these fixtures only, not product projects.

use super::{fixture, measured};
use crate::{
    Change, ChangeSet, Id, NoteGeometry, ObjectKey, ObjectKind, Project, Property, Tick, Value,
};
use std::sync::Arc;

fn diff(before: &Project, after: &Project, note_visits: usize) -> ChangeSet {
    let (changes, work) = measured(|| before.diff(after));
    let count = before.contents().len().max(after.contents().len());
    let key_limit = 14 * count * (count.checked_ilog2().unwrap_or(0) as usize + 1);
    assert!(
        work.content_keys <= key_limit,
        "changeset identity lookup inspected {} content keys for {count} contents (limit {key_limit})",
        work.content_keys
    );
    assert_eq!(
        work.diff_note_visits, note_visits,
        "changeset walked notes outside the changed content"
    );
    changes
}

fn round_trip(before: &Project, after: &Project, changes: &ChangeSet) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    assert_eq!(before.apply_changeset(changes).unwrap(), *after);
    assert_eq!(after.apply_changeset(&changes.invert()).unwrap(), *before);
}

#[test]
fn shared_content_identity_queries_use_logarithmic_work() {
    assert!(diff(&Project::new(), &Project::new(), 0).is_empty());
    for count in [64, 256] {
        let before = fixture(count);
        assert!(diff(&before, &before.clone(), 0).is_empty());
        let clip = before.tracks()[0].clips[count / 2].id;
        let after = before.set_clip_gain(clip, 0.5).unwrap();
        assert!(
            before
                .contents()
                .iter()
                .zip(after.contents())
                .all(|(left, right)| Arc::ptr_eq(left, right))
        );
        let changes = diff(&before, &after, 0);
        assert_eq!(
            changes.changes(),
            &[Change::Update {
                id: ObjectKey::Id(clip),
                property: Property::Gain,
                before: Value::Gain(1.0f32.to_bits()),
                after: Value::Gain(0.5f32.to_bits()),
            }]
        );
        round_trip(&before, &after, &changes);
    }
}

#[test]
fn note_diffs_only_walk_the_changed_content() {
    for count in [64, 256] {
        let before = fixture(count);
        for index in [0, count / 2, count - 1] {
            let track = &before.tracks()[0];
            let clip = &track.clips[index];
            let original = &before.content(clip.content).unwrap().notes[0];
            let after = before
                .edit_note(
                    track.id,
                    clip.id,
                    original.id,
                    NoteGeometry {
                        pitch: 61,
                        start: clip.start,
                        length: original.length,
                    },
                )
                .unwrap();
            // Two note-order reads, two note-map builds, and two diff passes.
            let changes = diff(&before, &after, 6);
            assert_eq!(
                changes.changes(),
                &[Change::Update {
                    id: ObjectKey::Id(original.id),
                    property: Property::Pitch,
                    before: Value::Byte(60),
                    after: Value::Byte(61),
                }]
            );
            for (old, new) in before.contents().iter().zip(after.contents()) {
                assert_eq!(Arc::ptr_eq(old, new), old.id != clip.content);
            }
            assert_eq!(original.pitch, 60);
            round_trip(&before, &after, &changes);
        }
    }
}

#[test]
fn independently_assembled_equal_contents_are_not_treated_as_shared() {
    for count in [64, 256] {
        let before = fixture(count);
        let after = Project::try_assemble(
            before.meter(),
            before.tempo().clone(),
            before
                .contents()
                .iter()
                .map(|content| (**content).clone())
                .rev()
                .collect(),
            before
                .tracks()
                .iter()
                .map(|track| (**track).clone())
                .collect(),
        )
        .unwrap();
        assert!(
            before
                .contents()
                .iter()
                .zip(after.contents())
                .all(|(left, right)| !Arc::ptr_eq(left, right))
        );
        assert!(diff(&before, &after, 6 * count).is_empty());
        assert_eq!(before, after);
    }
}

#[test]
fn content_creation_and_deletion_keep_notes_and_round_trip() {
    for count in [64, 256] {
        let before = fixture(count);
        let track = &before.tracks()[0];
        let source = track.clips.last().unwrap();
        // Replacing the first clip both retires a content and creates a new one.
        let after = before
            .duplicate_clip(source.id, track.id, Tick::new(0), Id::from_raw(100_000))
            .unwrap();
        let changes = diff(&before, &after, 6);
        for kind in [ObjectKind::Content, ObjectKind::Note, ObjectKind::Clip] {
            assert_eq!(
                changes
                    .changes()
                    .iter()
                    .filter(|change| matches!(change, Change::Create { kind: actual, .. } if *actual == kind))
                    .count(),
                1
            );
            assert_eq!(
                changes
                    .changes()
                    .iter()
                    .filter(|change| matches!(change, Change::Delete { kind: actual, .. } if *actual == kind))
                    .count(),
                1
            );
        }
        round_trip(&before, &after, &changes);
        let empty = Project::new();
        let changes = diff(&empty, &before, 3 * count);
        round_trip(&empty, &before, &changes);
        let changes = diff(&before, &empty, 3 * count);
        round_trip(&before, &empty, &changes);
    }
}
