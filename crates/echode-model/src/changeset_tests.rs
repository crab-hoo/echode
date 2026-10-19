use super::*;
use crate::model::NoteDraft;
use crate::time::TICKS_PER_QUARTER;

fn draft(start: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(start),
        length: Tick::new(TICKS_PER_QUARTER),
    }
}

#[test]
fn a_diff_round_trips_forward_and_back() {
    let before = Project::new();
    let (after, _) = before.add_track("piano");
    let changes = before.diff(&after);
    assert_eq!(before.apply_changeset(&changes).unwrap(), after);
    assert_eq!(after.apply_changeset(&changes.invert()).unwrap(), before);
}

#[test]
fn a_second_note_in_the_same_bar_does_not_rewrite_the_first() {
    let (project, track) = Project::new().add_track("piano");
    let project = project
        .insert_note_with_id(
            track,
            Id::from_raw(1),
            Id::from_raw(2),
            Id::from_raw(9),
            draft(0),
        )
        .unwrap();
    let next = project
        .insert_note_with_id(
            track,
            Id::from_raw(3),
            Id::from_raw(4),
            Id::from_raw(10),
            draft(TICKS_PER_QUARTER),
        )
        .unwrap();
    let changes = project.diff(&next);
    let rewrites_the_first = changes.changes().iter().any(|change| {
        matches!(
            change,
            Change::Update {
                id: ObjectKey::Id(id),
                ..
            } if *id == Id::from_raw(1)
        )
    });
    assert!(!rewrites_the_first);
    assert_eq!(project.apply_changeset(&changes).unwrap(), next);
    assert_eq!(next.apply_changeset(&changes.invert()).unwrap(), project);
}

#[test]
fn a_gain_edit_does_not_diff_unrelated_notes() {
    let (mut project, track) = Project::new().add_track("piano");
    for index in 0..32 {
        project = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(index * TICKS_PER_QUARTER),
                    length: Tick::new(TICKS_PER_QUARTER),
                },
            )
            .unwrap()
            .0;
    }
    let gain = project.tracks()[0].devices.last().unwrap().parameters[0].id;
    let next = project.set_parameter(gain, 0.5).unwrap();
    let changes = project.diff(&next);
    assert!(changes.changes().iter().all(|change| {
        !matches!(
            change,
            Change::Create {
                kind: ObjectKind::Note,
                ..
            } | Change::Delete {
                kind: ObjectKind::Note,
                ..
            } | Change::Update {
                property: Property::NoteOrder,
                ..
            }
        )
    }));
    assert_eq!(project.apply_changeset(&changes).unwrap(), next);
}

#[test]
fn one_added_note_records_an_order_step() {
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(TICKS_PER_QUARTER),
            },
        )
        .unwrap();
    let (next, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 64,
                velocity: 100,
                start: Tick::new(TICKS_PER_QUARTER),
                length: Tick::new(TICKS_PER_QUARTER),
            },
        )
        .unwrap();
    let changes = project.diff(&next);
    assert!(changes.changes().iter().any(|change| {
        matches!(
            change,
            Change::Update {
                property: Property::NoteOrder,
                after: Value::OrderStep { present: true, .. },
                ..
            }
        )
    }));
    assert!(changes.changes().iter().all(|change| {
        !matches!(
            change,
            Change::Update {
                property: Property::NoteOrder,
                after: Value::Ids(_),
                ..
            }
        )
    }));
    assert_eq!(project.apply_changeset(&changes).unwrap(), next);
    assert_eq!(next.apply_changeset(&changes.invert()).unwrap(), project);
}

#[test]
#[ignore = "timing measurement; `make spike-history` runs it in a release build"]
fn a_gain_edit_stays_cheap_as_unrelated_notes_grow() {
    for count in [1_000usize, 10_000, 100_000] {
        let (mut project, track) = Project::new().add_track("piano");
        for index in 0..count {
            project = project
                .insert_note(
                    track,
                    NoteDraft {
                        pitch: 60,
                        velocity: 100,
                        start: Tick::new(index as i64 * TICKS_PER_QUARTER),
                        length: Tick::new(1),
                    },
                )
                .unwrap()
                .0;
        }
        let gain = project.tracks()[0].devices.last().unwrap().parameters[0].id;
        let next = project.set_parameter(gain, 0.5).unwrap();
        let mut samples = [std::time::Duration::ZERO; 3];
        for sample in &mut samples {
            let started = std::time::Instant::now();
            let changes = project.diff(&next);
            *sample = started.elapsed();
            assert!(
                changes.changes().len() < 8,
                "{count} notes touched the score"
            );
        }
        samples.sort();
        println!(
            "gain diff, {count} unrelated notes: {} µs",
            samples[1].as_micros()
        );
        if count == 100_000 {
            assert!(
                samples[1] < std::time::Duration::from_millis(5),
                "100k notes took {:?}",
                samples[1]
            );
        }
    }
}
