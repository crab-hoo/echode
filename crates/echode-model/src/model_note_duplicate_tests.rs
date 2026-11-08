//! Copy budgets and fixture sizes constrain these tests, not product projects.

use super::{fixture, measured};
use crate::{ClipKind, Content, Id, ModelError, Project, Tick, Track};
use std::sync::Arc;

const COPY: Id = Id::from_raw(1_000_000);
const MISSING: Id = Id::from_raw(1_000_001);

pub(super) fn linked_fixture(notes: usize, kind: ClipKind) -> Project {
    let template = fixture(2);
    let bar = template.meter().bar_ticks().unwrap().get();
    let mut contents: Vec<_> = template
        .contents()
        .iter()
        .map(|item| Content::clone(item))
        .collect();
    let source = contents[0].notes[0].clone();
    contents[0].notes = (0..notes)
        .map(|index| {
            let mut note = source.clone();
            note.id = Id::from_raw(20_000 + index as u128);
            note.start = Tick::new(index as i64 * 2);
            note.length = Tick::new(1);
            note.velocity = 23_123;
            note.release = 54_321;
            note
        })
        .collect();
    let mut track = Track::clone(&template.tracks()[0]);
    track.clips[0].kind = kind;
    track.clips[0].length = Tick::new(if kind == ClipKind::Auto { bar } else { 100 });
    track.clips[0].gain = 0.25;
    track.clips[0].fade_in = 1;
    track.clips[0].fade_out = 2;
    track.clips[1].start = Tick::new(4 * bar);
    let (template, _) = template.add_track("linked");
    let mut linked = Track::clone(&template.tracks()[1]);
    let mut link = track.clips[0].clone();
    link.id = Id::from_raw(30_000);
    link.kind = ClipKind::Fixed;
    link.start = Tick::new(10 * bar);
    link.offset = Tick::new(-7);
    link.length = Tick::new(100);
    linked.clips.push(link);
    Project::try_assemble(
        template.meter(),
        template.tempo().clone(),
        contents,
        vec![track, linked],
    )
    .unwrap()
}

#[test]
fn duplication_copies_shared_content_only_once_before_refitting() {
    for count in [1, 64, 256] {
        for kind in [ClipKind::Fixed, ClipKind::Auto] {
            let project = linked_fixture(count, kind);
            let original = project.clone();
            let content = &project.contents()[0];
            let bar = project.meter().bar_ticks().unwrap().get();
            for track in project.tracks() {
                let clip = &track.clips[0];
                let origin = clip.start.get() - clip.offset.get();
                for source in [&content.notes[0], content.notes.last().unwrap()] {
                    for local in [-1, 10, bar + 5] {
                        let start = Tick::new(origin + local);
                        let (after, work) = measured(|| {
                            project
                                .duplicate_note(track.id, clip.id, source.id, COPY, 67, start)
                                .unwrap()
                        });
                        assert_eq!(
                            work.content_copies, 1,
                            "duplication cloned {} contents for {count} notes ({kind:?})",
                            work.content_copies
                        );
                        assert_eq!(work.content_note_copies, count);
                        let mut expected = Content::clone(content);
                        let mut copied = source.clone();
                        copied.id = COPY;
                        copied.pitch = 67;
                        copied.start = Tick::new(local);
                        expected.notes.push(copied);
                        crate::model::sort_notes(&mut expected.notes);
                        assert_eq!(after.content(content.id), Some(&expected));
                        assert!(!Arc::ptr_eq(
                            content,
                            after.shared_content(content.id).unwrap()
                        ));
                        assert!(Arc::ptr_eq(&project.contents()[1], &after.contents()[1]));
                        for (before_track, after_track) in
                            project.tracks().iter().zip(after.tracks())
                        {
                            for (before_clip, after_clip) in
                                before_track.clips.iter().zip(&after_track.clips)
                            {
                                if before_clip.kind == ClipKind::Fixed && before_clip.id != clip.id
                                {
                                    assert_eq!(before_clip, after_clip);
                                }
                                assert_eq!(before_clip.gain.to_bits(), after_clip.gain.to_bits());
                                assert_eq!(before_clip.fade_in, after_clip.fade_in);
                                assert_eq!(before_clip.fade_out, after_clip.fade_out);
                                assert_eq!(
                                    before_clip.start.get() - before_clip.offset.get(),
                                    after_clip.start.get() - after_clip.offset.get()
                                );
                            }
                        }
                        if clip.kind == ClipKind::Fixed {
                            let edited_track = after
                                .tracks()
                                .iter()
                                .find(|item| item.id == track.id)
                                .unwrap();
                            let edited_clip = edited_track
                                .clips
                                .iter()
                                .find(|item| item.id == clip.id)
                                .unwrap();
                            assert_eq!(edited_clip.start.get(), clip.start.get().min(start.get()));
                            assert_eq!(
                                edited_clip.start.get() + edited_clip.length.get(),
                                (clip.start.get() + clip.length.get()).max(start.get() + 1)
                            );
                        }
                        after.check_invariants().unwrap();
                        let changes = project.diff(&after);
                        assert_eq!(project.apply_changeset(&changes), Ok(after.clone()));
                        assert_eq!(
                            after.apply_changeset(&changes.invert()),
                            Ok(project.clone())
                        );
                        assert_eq!(project, original);
                    }
                }
            }
            project.check_invariants().unwrap();
        }
    }
}

#[test]
fn duplication_still_rejects_invalid_targets_without_changing_the_source() {
    let project = linked_fixture(64, ClipKind::Fixed);
    let original = project.clone();
    let track = &project.tracks()[0];
    let clip = &track.clips[0];
    let note = project.contents()[0].notes[0].id;
    for ((owner, placement, source), (copy, pitch, start), error) in [
        (
            (MISSING, clip.id, note),
            (COPY, 60, 0),
            ModelError::MissingTrack,
        ),
        (
            (track.id, MISSING, note),
            (COPY, 60, 0),
            ModelError::MissingClip,
        ),
        (
            (track.id, clip.id, MISSING),
            (COPY, 60, 0),
            ModelError::MissingNote,
        ),
        (
            (track.id, clip.id, project.contents()[1].notes[0].id),
            (COPY, 60, 0),
            ModelError::MissingNote,
        ),
        (
            (track.id, clip.id, note),
            (note, 60, 0),
            ModelError::DuplicateId,
        ),
        (
            (MISSING, MISSING, MISSING),
            (note, 128, i64::MAX),
            ModelError::DuplicateId,
        ),
        (
            (track.id, clip.id, note),
            (COPY, 128, 0),
            ModelError::InvalidNote,
        ),
        (
            (track.id, clip.id, note),
            (COPY, 60, i64::MAX),
            ModelError::InvalidNote,
        ),
    ] {
        assert_eq!(
            project.duplicate_note(owner, placement, source, copy, pitch, Tick::new(start)),
            Err(error)
        );
        assert_eq!(project, original);
    }
    project.check_invariants().unwrap();
}

#[test]
fn identical_geometry_duplication_still_sorts_the_new_note() {
    for count in [1, 64, 256] {
        for kind in [ClipKind::Fixed, ClipKind::Auto] {
            let project = linked_fixture(count, kind);
            let content = &project.contents()[0];
            for track in project.tracks() {
                let clip = &track.clips[0];
                for source in [&content.notes[0], content.notes.last().unwrap()] {
                    let (after, work) = measured(|| {
                        project
                            .duplicate_note(
                                track.id,
                                clip.id,
                                source.id,
                                COPY,
                                source.pitch,
                                clip.sounding_start(source).unwrap(),
                            )
                            .unwrap()
                    });
                    let mut expected = Content::clone(content);
                    let mut copied = source.clone();
                    copied.id = COPY;
                    expected.notes.push(copied);
                    crate::model::sort_notes(&mut expected.notes);
                    assert_eq!(
                        after.content(content.id),
                        Some(&expected),
                        "identical geometry duplication must sort the appended note"
                    );
                    assert_eq!(work.content_copies, 1);
                    assert_eq!(work.content_note_copies, count);
                    after.check_invariants().unwrap();
                    let changes = project.diff(&after);
                    assert_eq!(project.apply_changeset(&changes), Ok(after.clone()));
                    assert_eq!(
                        after.apply_changeset(&changes.invert()),
                        Ok(project.clone())
                    );
                }
            }
            project.check_invariants().unwrap();
        }
    }
}
