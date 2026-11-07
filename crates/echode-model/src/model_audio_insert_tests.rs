//! Audio placement work budgets and fixture sizes constrain these tests, not product projects.

use super::{fixture, measured};
use crate::{AudioRef, ClipKind, DEFAULT_FADE_FLICKS, Id, ModelError, Project, Tick};
use std::sync::Arc;

const CLIP: Id = Id::from_raw(1_000_000);
const CONTENT: Id = Id::from_raw(1_000_001);
const TAKE: Id = Id::from_raw(1_000_002);

fn audio() -> AudioRef {
    // Only reference metadata is used; no file, plugin, or audio device is opened.
    AudioRef::try_new("ab".repeat(32), 48_000, 2, 48_000).unwrap()
}

fn populated(count: usize) -> Project {
    let project = fixture(count).add_track("unrelated").0;
    let mut contents: Vec<_> = project
        .contents()
        .iter()
        .map(|item| (**item).clone())
        .collect();
    contents[0].takes.push(TAKE);
    Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        contents,
        project
            .tracks()
            .iter()
            .map(|item| (**item).clone())
            .collect(),
    )
    .unwrap()
}

fn place(
    project: &Project,
    clip: Id,
    content: Id,
    explicit_fades: bool,
) -> Result<Project, ModelError> {
    let track = project.tracks()[0].id;
    let start = Tick::new(2 * project.meter().bar_ticks().unwrap().get());
    if explicit_fades {
        project.place_audio_with_fades(track, clip, content, start, audio(), 123, 0)
    } else {
        project.place_audio(track, clip, content, start, audio())
    }
}

fn round_trip(before: &Project, after: &Project) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    let changes = before.diff(after);
    assert_eq!(before.apply_changeset(&changes), Ok(after.clone()));
    assert_eq!(after.apply_changeset(&changes.invert()), Ok(before.clone()));
}

#[test]
fn audio_placements_check_candidate_ids_in_one_pass() {
    for count in [64, 256] {
        let project = populated(count);
        let original = project.clone();
        for explicit_fades in [false, true] {
            let (result, work) = measured(|| place(&project, CLIP, CONTENT, explicit_fades));
            let after = result.unwrap();
            assert_eq!(
                work.id_check_notes, count,
                "audio placement scanned {} note IDs for {count} stored notes; candidate IDs must be checked together",
                work.id_check_notes,
            );
            assert_eq!(work.id_checks, 1);
            assert_eq!(work.used_id_collections, 0);
            let clip = after.tracks()[0].clips.last().unwrap();
            assert_eq!(clip.id, CLIP);
            assert_eq!(clip.content, CONTENT);
            assert_eq!(clip.kind, ClipKind::Fixed);
            assert_eq!(
                clip.start.get(),
                2 * project.meter().bar_ticks().unwrap().get()
            );
            assert_eq!(clip.length, Tick::new(2 * crate::TICKS_PER_QUARTER));
            assert_eq!(clip.offset, Tick::new(0));
            assert_eq!(clip.gain, 1.0);
            let fades = if explicit_fades {
                (123, 0)
            } else {
                (DEFAULT_FADE_FLICKS, DEFAULT_FADE_FLICKS)
            };
            assert_eq!((clip.fade_in, clip.fade_out), fades);
            let content = after.content(CONTENT).unwrap();
            assert_eq!(content.audio, Some(audio()));
            assert!(content.notes.is_empty());
            assert!(content.takes.is_empty());
            assert!(content.expressions.is_empty());
            for content in project.contents() {
                assert!(Arc::ptr_eq(
                    content,
                    after.shared_content(content.id).unwrap()
                ));
            }
            assert!(Arc::ptr_eq(&project.tracks()[1], &after.tracks()[1]));
            assert_eq!(project, original);
            round_trip(&project, &after);
        }
    }
}

#[test]
fn audio_placement_rejects_collisions_with_every_stored_identity() {
    let project = populated(2);
    let original = project.clone();
    let content = &project.contents()[0];
    let track = &project.tracks()[0];
    let occupied = [
        content.id,
        content.notes[0].id,
        TAKE,
        track.id,
        track.clips[0].id,
        track.devices[0].id,
    ];
    let occupied = occupied.into_iter().chain(
        track.devices[0]
            .parameters
            .iter()
            .map(|parameter| parameter.id),
    );
    for used in occupied {
        for explicit_fades in [false, true] {
            for (clip, content) in [(used, CONTENT), (CLIP, used)] {
                let (result, work) = measured(|| place(&project, clip, content, explicit_fades));
                assert_eq!(result, Err(ModelError::DuplicateId));
                assert_eq!(work.id_checks, 1);
                assert!(work.id_check_notes <= 2);
                assert_eq!(project, original);
            }
        }
    }
    let (result, work) = measured(|| place(&project, CLIP, CLIP, false));
    assert_eq!(result, Err(ModelError::InvalidClip));
    assert_eq!(work.id_checks, 0);
    assert_eq!(project, original);
}

#[test]
fn audio_placement_candidates_accept_both_orders_and_integer_endpoints() {
    let project = populated(2);
    for (clip, content) in [(0, u128::MAX), (u128::MAX, 0), (1_000_001, 1_000_000)] {
        let after = place(&project, Id::from_raw(clip), Id::from_raw(content), true).unwrap();
        assert_eq!(
            after.tracks()[0].clips.last().unwrap().id,
            Id::from_raw(clip)
        );
        assert_eq!(
            after.content(Id::from_raw(content)).unwrap().audio,
            Some(audio())
        );
        round_trip(&project, &after);
    }
}

#[test]
fn invalid_audio_placements_preserve_error_precedence_and_original_snapshot() {
    let project = populated(2);
    let original = project.clone();
    let track = project.tracks()[0].id;
    let missing = Id::from_raw(0);
    let used = project.contents()[0].notes[0].id;
    for ((owner, clip, content), (start, frames, fade), (expected, checks)) in [
        (
            (missing, used, used),
            (-1, 0, -1),
            (ModelError::InvalidAudio, 0),
        ),
        (
            (missing, used, used),
            (0, 48_000, 0),
            (ModelError::InvalidClip, 0),
        ),
        (
            (missing, used, CONTENT),
            (-1, 48_000, 0),
            (ModelError::InvalidClip, 0),
        ),
        (
            (missing, used, CONTENT),
            (0, 48_000, -1),
            (ModelError::InvalidClip, 0),
        ),
        (
            (missing, used, CONTENT),
            (0, 48_000, 0),
            (ModelError::DuplicateId, 1),
        ),
        (
            (missing, CLIP, used),
            (0, 48_000, 0),
            (ModelError::DuplicateId, 1),
        ),
        (
            (missing, CLIP, CONTENT),
            (0, 48_000, 0),
            (ModelError::MissingTrack, 1),
        ),
        (
            (track, CLIP, CONTENT),
            (0, 48_000, 0),
            (ModelError::Overlap, 1),
        ),
        (
            (track, CLIP, CONTENT),
            (i64::MAX, 48_000, 0),
            (ModelError::InvalidClip, 1),
        ),
    ] {
        let mut reference = audio();
        reference.frames = frames;
        let (result, work) = measured(|| {
            project.place_audio_with_fades(
                owner,
                clip,
                content,
                Tick::new(start),
                reference,
                fade,
                0,
            )
        });
        assert_eq!(result, Err(expected));
        assert_eq!(work.id_checks, checks);
        assert_eq!(project, original);
    }
    project.check_invariants().unwrap();
}
