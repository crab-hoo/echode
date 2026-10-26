use super::*;
use crate::model::{AudioRef, NoteDraft, NoteGeometry};
use crate::time::TICKS_PER_QUARTER;

const BAR: i64 = TICKS_PER_QUARTER * 4;
const AUDIO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn audio(seconds: u64) -> AudioRef {
    AudioRef::try_new(AUDIO.into(), 48_000, 2, 48_000 * seconds).unwrap()
}

fn clips(project: &Project, track: Id) -> Vec<Placement> {
    project
        .tracks()
        .iter()
        .find(|candidate| candidate.id == track)
        .unwrap()
        .clips
        .clone()
}

fn note(start: i64, length: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(start),
        length: Tick::new(length),
    }
}

/// Two tracks. At 120 bpm one second is two quarters, so audio of
/// `seconds` spans `seconds * 2` quarters.
fn two_tracks() -> (Project, Id, Id) {
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    (project, first, second)
}

#[test]
fn a_move_to_another_track_keeps_identity_window_and_mix() {
    let (project, first, second) = two_tracks();
    let (clip, content) = (Id::from_raw(10), Id::from_raw(11));
    let project = project
        .place_audio(first, clip, content, Tick::new(0), audio(1))
        .unwrap()
        .set_clip_gain(clip, 0.5)
        .unwrap()
        .set_clip_fades(clip, 0, 7_056_000)
        .unwrap();
    let before = clips(&project, first)[0].clone();
    let start = Tick::new(TICKS_PER_QUARTER * 3 + 17);
    let moved = project
        .move_clip(clip, second, start, Id::from_raw(1))
        .unwrap();
    moved.check_invariants().unwrap();
    assert!(clips(&moved, first).is_empty());
    let after = clips(&moved, second);
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0],
        Placement {
            start,
            ..before.clone()
        }
    );
    // The diff records an owner change, and replaying it rebuilds the move.
    assert_eq!(project.apply_changeset(&project.diff(&moved)), Ok(moved));
}

#[test]
fn a_landing_window_removes_trims_and_splits_on_the_target_track() {
    let (project, first, second) = two_tracks();
    // Target: [0, 4q) [4q, 8q) [8q, 12q) on `second`, and one long clip on `first`.
    let project = project
        .place_audio(
            second,
            Id::from_raw(20),
            Id::from_raw(21),
            Tick::new(0),
            audio(2),
        )
        .unwrap()
        .place_audio(
            second,
            Id::from_raw(22),
            Id::from_raw(23),
            Tick::new(BAR),
            audio(2),
        )
        .unwrap()
        .place_audio(
            second,
            Id::from_raw(24),
            Id::from_raw(25),
            Tick::new(BAR * 2),
            audio(2),
        )
        .unwrap()
        .place_audio(
            first,
            Id::from_raw(26),
            Id::from_raw(27),
            Tick::new(0),
            audio(4),
        )
        .unwrap()
        .set_clip_fades(Id::from_raw(20), 1, 2)
        .unwrap()
        .set_clip_fades(Id::from_raw(24), 3, 4)
        .unwrap();
    // One quarter into the first clip, covering the second, ending one quarter into the third.
    let window = project
        .resize_clip(
            first,
            Id::from_raw(26),
            Tick::new(0),
            Tick::new(BAR + TICKS_PER_QUARTER * 2),
        )
        .unwrap();
    let start = Tick::new(TICKS_PER_QUARTER * 3);
    let moved = window
        .move_clip(Id::from_raw(26), second, start, Id::from_raw(2))
        .unwrap();
    moved.check_invariants().unwrap();
    let after = clips(&moved, second);
    let summary: Vec<_> = after
        .iter()
        .map(|clip| {
            (
                clip.id,
                clip.start.get(),
                clip.length.get(),
                clip.offset.get(),
                clip.fade_in,
                clip.fade_out,
            )
        })
        .collect();
    let end = TICKS_PER_QUARTER * 9;
    assert_eq!(
        summary,
        vec![
            (
                Id::from_raw(20),
                0,
                TICKS_PER_QUARTER * 3,
                0,
                1,
                DEFAULT_FADE_FLICKS
            ),
            (
                Id::from_raw(26),
                start.get(),
                BAR + TICKS_PER_QUARTER * 2,
                0,
                DEFAULT_FADE_FLICKS,
                DEFAULT_FADE_FLICKS
            ),
            (
                Id::from_raw(24),
                end,
                BAR * 3 - end,
                end - BAR * 2,
                DEFAULT_FADE_FLICKS,
                4
            ),
        ]
    );
    // The covered clip's content went with it.
    assert!(moved.content(Id::from_raw(23)).is_none());
    assert!(after.iter().all(|clip| clip.kind == ClipKind::Fixed));

    // A short window inside one clip splits it into two pieces on one content.
    let short = window
        .resize_clip(
            first,
            Id::from_raw(26),
            Tick::new(0),
            Tick::new(TICKS_PER_QUARTER),
        )
        .unwrap();
    let split = short
        .move_clip(
            Id::from_raw(26),
            second,
            Tick::new(BAR + 5),
            Id::from_raw(3),
        )
        .unwrap();
    split.check_invariants().unwrap();
    let pieces: Vec<_> = clips(&split, second)
        .into_iter()
        .filter(|clip| clip.content == Id::from_raw(23))
        .collect();
    assert_eq!(pieces.len(), 2);
    assert_eq!(pieces[0].id, Id::from_raw(22));
    assert_eq!((pieces[0].start.get(), pieces[0].length.get()), (BAR, 5));
    let right_start = BAR + 5 + TICKS_PER_QUARTER;
    assert_eq!(
        (
            pieces[1].start.get(),
            pieces[1].offset.get(),
            pieces[1].length.get()
        ),
        (right_start, right_start - BAR, BAR * 2 - right_start)
    );
    assert_eq!(
        pieces[1].id,
        derived_id(SPLIT_CONTEXT, Id::from_raw(3), Id::from_raw(22))
    );
    assert_eq!(
        (pieces[0].fade_out, pieces[1].fade_in),
        (DEFAULT_FADE_FLICKS, DEFAULT_FADE_FLICKS)
    );
    // Replaying the same edit derives the same piece id.
    assert_eq!(
        short.move_clip(
            Id::from_raw(26),
            second,
            Tick::new(BAR + 5),
            Id::from_raw(3)
        ),
        Ok(split.clone())
    );
    assert_eq!(short.apply_changeset(&short.diff(&split)), Ok(split));
}

#[test]
fn a_move_on_its_own_track_may_overlap_where_it_was() {
    let (project, first, _) = two_tracks();
    let project = project
        .place_audio(
            first,
            Id::from_raw(30),
            Id::from_raw(31),
            Tick::new(0),
            audio(2),
        )
        .unwrap()
        .place_audio(
            first,
            Id::from_raw(32),
            Id::from_raw(33),
            Tick::new(BAR),
            audio(2),
        )
        .unwrap();
    let moved = project
        .move_clip(
            Id::from_raw(30),
            first,
            Tick::new(TICKS_PER_QUARTER * 2),
            Id::from_raw(4),
        )
        .unwrap();
    moved.check_invariants().unwrap();
    let after = clips(&moved, first);
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].id, Id::from_raw(30));
    assert_eq!(after[1].id, Id::from_raw(32));
    assert_eq!(after[1].start.get(), TICKS_PER_QUARTER * 6);
}

#[test]
fn an_auto_clip_stays_auto_on_sub_bar_moves_too() {
    let (project, first, second) = two_tracks();
    let (project, _) = project
        .insert_note(first, note(TICKS_PER_QUARTER, TICKS_PER_QUARTER))
        .unwrap();
    let clip = clips(&project, first)[0].clone();
    assert_eq!(clip.kind, ClipKind::Auto);
    let bars = project
        .move_clip(clip.id, second, Tick::new(BAR * 3), Id::from_raw(5))
        .unwrap();
    bars.check_invariants().unwrap();
    assert_eq!(clips(&bars, second)[0].kind, ClipKind::Auto);
    // Notes keep following an auto clip after it moved.
    let (grown, _) = bars
        .insert_note(second, note(BAR * 4 + TICKS_PER_QUARTER, TICKS_PER_QUARTER))
        .unwrap();
    grown.check_invariants().unwrap();
    assert_eq!(clips(&grown, second)[0].length.get(), BAR * 2);

    let beat = project
        .move_clip(
            clip.id,
            first,
            Tick::new(TICKS_PER_QUARTER),
            Id::from_raw(6),
        )
        .unwrap();
    beat.check_invariants().unwrap();
    let moved = &clips(&beat, first)[0];
    assert_eq!(moved.kind, ClipKind::Auto);
    assert_eq!(moved.start.get(), TICKS_PER_QUARTER);
    // The note sounds one quarter later, with the clip.
    let note = &beat.content(moved.content).unwrap().notes[0];
    assert_eq!(
        moved.sounding_start(note).unwrap().get(),
        TICKS_PER_QUARTER * 2
    );
}

#[test]
fn a_shifted_auto_refit_refuses_overflow_without_mutating_the_source() {
    let (project, first, second) = two_tracks();
    let (project, note_id) = project
        .insert_note(first, note(TICKS_PER_QUARTER, TICKS_PER_QUARTER))
        .unwrap();
    let clip = clips(&project, first)[0].id;
    let start = i64::MAX - BAR - TICKS_PER_QUARTER / 2;
    let shifted = project
        .move_clip(clip, second, Tick::new(start), Id::from_raw(90))
        .unwrap();
    let before = shifted.clone();
    // The note itself fits, but rounding its end to a content bar does not.
    assert_eq!(
        shifted.edit_note(
            second,
            clip,
            note_id,
            NoteGeometry {
                pitch: 60,
                start: Tick::new(start + TICKS_PER_QUARTER * 3),
                length: Tick::new(TICKS_PER_QUARTER + 1),
            },
        ),
        Err(ModelError::InvalidNote)
    );
    assert_eq!(shifted, before);
    shifted.check_invariants().unwrap();
}

#[test]
fn a_duplicate_owns_new_content_and_notes() {
    let (project, first, second) = two_tracks();
    let (project, original_note) = project
        .insert_note(first, note(0, TICKS_PER_QUARTER))
        .unwrap();
    let (project, _) = project
        .insert_note(first, note(TICKS_PER_QUARTER, TICKS_PER_QUARTER))
        .unwrap();
    let source = clips(&project, first)[0].clone();
    let seed = Id::from_raw(7);
    let copied = project
        .duplicate_clip(source.id, second, Tick::new(BAR * 2), seed)
        .unwrap();
    copied.check_invariants().unwrap();
    assert_eq!(clips(&copied, first), vec![source.clone()]);
    let copy = clips(&copied, second)[0].clone();
    assert_eq!(copy.id, derived_id(COPY_CONTEXT, seed, source.id));
    assert_eq!(copy.content, derived_id(COPY_CONTEXT, seed, source.content));
    assert_eq!(copy.kind, ClipKind::Auto);
    let notes = &copied.content(copy.content).unwrap().notes;
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().all(|note| note.id != original_note));
    // Editing the copy leaves the original's content alone.
    let edited = copied
        .edit_note(
            second,
            copy.id,
            derived_id(COPY_CONTEXT, seed, original_note),
            NoteGeometry {
                pitch: 72,
                start: Tick::new(0),
                length: Tick::new(TICKS_PER_QUARTER),
            },
        )
        .unwrap();
    assert_eq!(
        edited.content(source.content),
        project.content(source.content)
    );
    assert_eq!(
        project.duplicate_clip(source.id, second, Tick::new(BAR * 2), seed),
        Ok(copied.clone())
    );
    assert_eq!(project.apply_changeset(&project.diff(&copied)), Ok(copied));

    // Copying onto the original's own track replaces the part it covers.
    let over = project
        .duplicate_clip(source.id, first, Tick::new(TICKS_PER_QUARTER * 2), seed)
        .unwrap();
    over.check_invariants().unwrap();
    let after = clips(&over, first);
    assert_eq!(after.len(), 2);
    assert_eq!(
        (after[0].id, after[0].length.get(), after[0].kind),
        (source.id, TICKS_PER_QUARTER * 2, ClipKind::Fixed)
    );
    assert_eq!(after[1].kind, ClipKind::Auto);
}

#[test]
fn an_audio_duplicate_shares_the_media_but_not_the_content() {
    let (project, first, _) = two_tracks();
    let project = project
        .place_audio(
            first,
            Id::from_raw(40),
            Id::from_raw(41),
            Tick::new(0),
            audio(1),
        )
        .unwrap();
    let copied = project
        .duplicate_clip(Id::from_raw(40), first, Tick::new(BAR), Id::from_raw(8))
        .unwrap();
    copied.check_invariants().unwrap();
    let after = clips(&copied, first);
    assert_eq!(after.len(), 2);
    assert_ne!(after[1].content, Id::from_raw(41));
    assert_eq!(
        copied.content(after[1].content).unwrap().audio,
        copied.content(Id::from_raw(41)).unwrap().audio
    );
}

#[test]
fn refused_moves_name_the_reason() {
    let (project, first, second) = two_tracks();
    let project = project
        .place_audio(
            first,
            Id::from_raw(50),
            Id::from_raw(51),
            Tick::new(0),
            audio(1),
        )
        .unwrap();
    let seed = Id::from_raw(9);
    for edit in [Project::move_clip, Project::duplicate_clip] {
        assert_eq!(
            edit(&project, Id::from_raw(50), second, Tick::new(-1), seed),
            Err(ModelError::InvalidClip)
        );
        assert_eq!(
            edit(
                &project,
                Id::from_raw(50),
                second,
                Tick::new(i64::MAX - 1),
                seed
            ),
            Err(ModelError::InvalidClip)
        );
        assert_eq!(
            edit(&project, Id::from_raw(99), second, Tick::new(0), seed),
            Err(ModelError::MissingClip)
        );
        assert_eq!(
            edit(
                &project,
                Id::from_raw(50),
                Id::from_raw(98),
                Tick::new(0),
                seed
            ),
            Err(ModelError::MissingTrack)
        );
    }
    // A window cut far before its notes cannot land where their time would overflow.
    let (notes, track) = Project::new().add_track("notes");
    let (notes, _) = notes.insert_note(track, note(BAR * 100, 1)).unwrap();
    let clip = clips(&notes, track)[0].id;
    let early = notes
        .resize_clip(track, clip, Tick::new(0), Tick::new(1))
        .unwrap();
    assert_eq!(
        early.move_clip(clip, track, Tick::new(i64::MAX - 1), seed),
        Err(ModelError::InvalidClip)
    );
    assert!(early.move_clip(clip, track, Tick::new(BAR), seed).is_ok());
}
