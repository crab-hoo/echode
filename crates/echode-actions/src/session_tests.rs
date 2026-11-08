use std::mem::size_of;

use super::*;
use crate::action::{Action, ActionError};
use crate::journal::{decode, encode};
use crate::{Command, catalog};
use echode_model::{Id, Meter, ModelError, NoteDraft, TICKS_PER_QUARTER, Tempo, Tick};

fn quarter(start: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::quarters(start).unwrap(),
        length: Tick::new(TICKS_PER_QUARTER),
    }
}

#[test]
fn no_op_commands_and_cancelled_groups_preserve_session_history() {
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack { name: "a".into() })
        .unwrap();
    session.undo().unwrap();
    let base = session.project().clone();
    let meter = base.meter();
    session.perform(Command::SetMeter { meter }).unwrap();
    session
        .transaction(|tx| {
            tx.perform(Command::SetMeter { meter })?;
            Ok(())
        })
        .unwrap();
    session
        .transaction(|tx| {
            tx.perform(Command::SetMeter {
                meter: Meter::try_new(3, 4).unwrap(),
            })?;
            tx.perform(Command::SetMeter { meter })?;
            Ok(())
        })
        .unwrap();
    assert_eq!(session.project(), &base);
    assert!(session.journal().is_empty(), "no-op session journal");
    assert_eq!(session.undo().unwrap_err(), ActionError::NothingToUndo);
    session
        .redo()
        .expect("no-op session edits must preserve redo");
    assert_eq!(session.project().tracks()[0].name, "a");
    session.undo().unwrap();
    assert_eq!(session.project(), &base);
}

#[test]
fn undo_and_redo_roundtrip_a_note() {
    let base = Project::new();
    let mut session = Session::open(base.clone());
    let Action::AddTrack { id: track, .. } = session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap()
    else {
        panic!("add track");
    };
    session
        .perform(Command::InsertNote {
            track,
            draft: quarter(0),
        })
        .unwrap();
    assert_eq!(session.project().contents()[0].notes.len(), 1);

    session.undo().unwrap();
    assert!(session.project().tracks()[0].clips.is_empty());
    session.undo().unwrap();
    assert!(session.project().tracks().is_empty());
    assert_eq!(session.undo().unwrap_err(), ActionError::NothingToUndo);

    session.redo().unwrap();
    session.redo().unwrap();
    assert_eq!(session.project().contents()[0].notes.len(), 1);
    assert_eq!(
        replay(&base, session.journal()).unwrap(),
        *session.project()
    );
    assert_eq!(session.redo().unwrap_err(), ActionError::NothingToRedo);
}

#[test]
fn a_new_edit_drops_the_redo_branch() {
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    session.undo().unwrap();
    session
        .perform(Command::AddTrack {
            name: "drums".into(),
        })
        .unwrap();
    assert_eq!(session.redo().unwrap_err(), ActionError::NothingToRedo);
    assert_eq!(session.project().tracks()[0].name, "drums");
}

#[test]
fn a_transaction_is_one_undo_step() {
    let base = Project::new();
    let mut session = Session::open(base.clone());
    session
        .transaction(|tx| {
            let Action::AddTrack { id: track, .. } = tx.perform(Command::AddTrack {
                name: "piano".into(),
            })?
            else {
                return Err(ActionError::CorruptJournal);
            };
            tx.perform(Command::InsertNote {
                track,
                draft: quarter(0),
            })?;
            tx.perform(Command::InsertNote {
                track,
                draft: quarter(4),
            })?;
            Ok(())
        })
        .unwrap();
    assert_eq!(session.journal().len(), 1);
    assert_eq!(session.project().contents()[0].notes.len(), 2);

    session.undo().unwrap();
    assert!(session.project().tracks().is_empty());
    assert_eq!(
        replay(&base, session.journal()).unwrap(),
        *session.project()
    );
    session.redo().unwrap();
    assert_eq!(
        session.project().tracks()[0].clips[0].length.get(),
        TICKS_PER_QUARTER * 8
    );
}

#[test]
fn a_failed_transaction_restores_the_redo_stack() {
    let base = Project::new();
    let mut session = Session::open(base.clone());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    session.undo().unwrap();

    let error = session
        .transaction(|tx| {
            let Action::AddTrack { id: track, .. } = tx.perform(Command::AddTrack {
                name: "drums".into(),
            })?
            else {
                return Err(ActionError::CorruptJournal);
            };
            tx.perform(Command::InsertNote {
                track,
                draft: NoteDraft {
                    pitch: 128,
                    ..quarter(0)
                },
            })?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error, ActionError::Model(ModelError::InvalidNote));
    assert!(session.project().tracks().is_empty());
    assert!(session.journal().is_empty());

    session.redo().unwrap();
    assert_eq!(session.project().tracks()[0].name, "piano");
    assert_eq!(
        replay(&base, session.journal()).unwrap(),
        *session.project()
    );
}

#[test]
fn resize_survives_journal_replay() {
    let base = Project::new();
    let mut session = Session::open(base.clone());
    let Action::AddTrack { id: track, .. } = session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap()
    else {
        panic!("add track");
    };
    let Action::InsertNote { clip, .. } = session
        .perform(Command::InsertNote {
            track,
            draft: quarter(1),
        })
        .unwrap()
    else {
        panic!("insert");
    };
    session
        .perform(Command::ResizeClip {
            track,
            clip,
            start: Tick::quarters(1).unwrap(),
            length: Tick::quarters(4).unwrap(),
        })
        .unwrap();
    session
        .perform(Command::SetTempo {
            at: Tick::new(0),
            tempo: Tempo::quarters_per_minute(100).unwrap(),
        })
        .unwrap();

    let text = encode(session.journal());
    let restored = replay(&base, &decode(&text).unwrap()).unwrap();
    assert_eq!(restored, *session.project());
    assert_eq!(restored.tracks()[0].clips[0].id, clip);
    assert_eq!(
        restored.tracks()[0].clips[0].kind,
        echode_model::ClipKind::Fixed
    );

    session.undo().unwrap();
    assert_eq!(
        replay(&base, session.journal()).unwrap(),
        *session.project()
    );
}

#[test]
fn changing_the_meter_after_a_note_does_not_record_a_step() {
    let mut session = Session::open(Project::new());
    let Action::AddTrack { id: track, .. } = session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap()
    else {
        panic!("add track");
    };
    session
        .perform(Command::InsertNote {
            track,
            draft: quarter(0),
        })
        .unwrap();
    let before = session.project().clone();
    let error = session
        .perform(Command::SetMeter {
            meter: Meter::try_new(3, 4).unwrap(),
        })
        .unwrap_err();
    assert_eq!(error, ActionError::Model(ModelError::InvalidMeter));
    assert_eq!(*session.project(), before);
    assert_eq!(session.journal().len(), 2);
}

#[test]
fn a_child_track_undoes_and_replays() {
    let base = Project::new();
    let mut session = Session::open(base.clone());
    let Action::AddTrack { id: folder, .. } = session
        .perform(Command::AddTrack {
            name: "drums".into(),
        })
        .unwrap()
    else {
        panic!("add track");
    };
    let Action::AddChild { id: kick, .. } = session
        .perform(Command::AddChild {
            parent: folder,
            name: "kick".into(),
        })
        .unwrap()
    else {
        panic!("add child");
    };
    session
        .perform(Command::SetParent {
            track: kick,
            parent: None,
        })
        .unwrap();
    assert_eq!(session.project().parent(kick).unwrap(), None);
    assert_eq!(
        replay(&base, &decode(&encode(session.journal())).unwrap()).unwrap(),
        *session.project()
    );
    session.undo().unwrap();
    assert_eq!(session.project().parent(kick).unwrap(), Some(folder));
    assert_eq!(
        session
            .perform(Command::SetParent {
                track: folder,
                parent: Some(kick),
            })
            .unwrap_err(),
        ActionError::Model(ModelError::Cycle)
    );
    assert_eq!(session.project().parent(kick).unwrap(), Some(folder));
}

#[test]
fn recovery_applies_the_changeset_and_leaves_the_command_alone() {
    let base = Project::new();
    let mut session = Session::open(base.clone());
    let Action::AddTrack { id, .. } = session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap()
    else {
        panic!("add track");
    };
    let mut entry = session.journal()[0].clone();
    let Action::AddTrack {
        device,
        gain,
        pan,
        mute,
        solo,
        ..
    } = session.journal()[0].actions[0].clone()
    else {
        panic!("add track");
    };
    entry.actions = vec![Action::AddTrack {
        id,
        name: "renamed by a later rule".into(),
        device,
        gain,
        pan,
        mute,
        solo,
    }];
    let restored = replay(&base, &[entry]).unwrap();
    assert_eq!(restored.tracks()[0].name, "piano");
}

#[test]
fn the_fader_undoes_without_moving_the_other_mixer_values() {
    let mut session = Session::open(Project::new());
    let Action::AddTrack {
        gain,
        pan,
        mute,
        solo,
        ..
    } = session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap()
    else {
        panic!("add track");
    };
    let pan_bits = session.project().parameter(pan).unwrap().value.to_bits();
    session
        .perform(Command::SetParameter {
            parameter: gain,
            value: 0.0,
        })
        .unwrap();
    assert_eq!(session.project().parameter(gain).unwrap().value, 0.0);
    assert_eq!(
        session.project().parameter(pan).unwrap().value.to_bits(),
        pan_bits
    );
    assert_eq!(session.project().parameter(mute).unwrap().value, 0.0);
    assert_eq!(session.project().parameter(solo).unwrap().value, 0.0);
    session.undo().unwrap();
    assert_eq!(session.project().parameter(gain).unwrap().value, 1.0);
}

#[test]
fn a_reverted_commit_restores_the_project_without_copying_history() {
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    let parked = session.park_redo();
    session
        .perform(Command::AddTrack {
            name: "bass".into(),
        })
        .unwrap();
    assert_eq!(session.project().tracks().len(), 2);
    session.revert_commit(parked).unwrap();
    assert_eq!(session.project().tracks().len(), 1);
    assert_eq!(session.journal().len(), 1);
}

#[test]
fn restoring_a_snapshot_undoes_and_replays_from_the_changeset() {
    let base = Project::new();
    let mut session = Session::open(base.clone());
    session
        .perform(Command::AddTrack { name: "one".into() })
        .unwrap();
    let earlier = session.project().clone();
    session
        .perform(Command::AddTrack { name: "two".into() })
        .unwrap();
    let current = session.project().clone();
    assert!(session.restore(current, "same".into()).unwrap().is_empty());
    session.restore(earlier.clone(), "abc".into()).unwrap();
    assert_eq!(session.project(), &earlier);
    let replayed = replay(&base, &decode(&encode(session.journal())).unwrap()).unwrap();
    assert_eq!(replayed, earlier);
    session.undo().unwrap();
    assert_eq!(session.project().tracks().len(), 2);
    session
        .perform(Command::Restore {
            revision: "abc".into(),
        })
        .unwrap_err();
    assert_eq!(session.project().tracks().len(), 2);
}

#[test]
fn catalog_covers_each_action_once() {
    let ids: Vec<_> = catalog().iter().map(|spec| spec.id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len());
    assert!(catalog().iter().all(|spec| {
        !spec.name_ja.is_empty()
            && !spec.name_en.is_empty()
            && !spec.description_ja.is_empty()
            && !spec.description_en.is_empty()
    }));

    let sample = Action::DeleteNote {
        track: Id::from_raw(1),
        note: Id::from_raw(2),
    };
    assert_eq!(sample.spec().id, "note.delete");
}

#[test]
fn audit_rea07_a_refused_shared_edit_leaves_the_journal() {
    let mut session = Session::open(Project::new());
    let Action::AddTrack { id: track, .. } = session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Action::InsertNote { clip, note, .. } = session
        .perform(Command::InsertNote {
            track,
            draft: NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(1),
            },
        })
        .unwrap()
    else {
        panic!("note");
    };
    session
        .perform(Command::ResizeClip {
            track,
            clip,
            start: Tick::new(0),
            length: Tick::new(2),
        })
        .unwrap();
    let Action::AddTrack { id: other, .. } = session
        .perform(Command::AddTrack {
            name: "link".into(),
        })
        .unwrap()
    else {
        panic!("link track");
    };
    session
        .perform(Command::LinkClip {
            source: clip,
            track: other,
            start: Tick::new(i64::MAX - 3),
        })
        .unwrap();
    let before = session.project().clone();
    let journal = session.journal().len();
    let error = session.perform(Command::EditNote {
        track,
        clip,
        note,
        pitch: 60,
        start: Tick::new(0),
        length: Tick::new(8),
    });
    assert!(error.is_err());
    assert_eq!(session.project(), &before);
    assert_eq!(session.journal().len(), journal);
    assert!(session.project().check_invariants().is_ok());
}

/// One track with `notes` quarter notes, and nothing on the undo stack.
fn session_with_notes(notes: i64) -> Session {
    let (project, track) = Project::new().add_track("a");
    let mut project = project;
    for start in 0..notes {
        project = project.insert_note(track, quarter(start)).unwrap().0;
    }
    Session::open(project)
}

fn set_velocity(session: &mut Session, index: usize, velocity: u8) {
    let project = session.project();
    let track = project.tracks()[0].id;
    let clip = project.tracks()[0].clips[0].id;
    let note = project.contents()[0].notes[index].id;
    session
        .perform(Command::SetNoteVelocity {
            track,
            clip,
            note,
            velocity,
        })
        .unwrap();
}

#[test]
fn usage_counts_each_replaced_content_once_wherever_current_is() {
    let notes = 64;
    let mut session = session_with_notes(notes);
    assert_eq!(session.usage(), HistoryUsage::default());
    for step in 0..6 {
        set_velocity(&mut session, step, 1 + step as u8);
    }
    let edited = session.usage();
    assert_eq!((edited.undo_steps, edited.redo_steps), (6, 0));
    // A velocity edit copies the whole content, so each step keeps all notes.
    assert_eq!(edited.versions.contents, 6);
    assert_eq!(edited.versions.notes, 6 * notes as u64);
    assert_eq!(edited.versions.tracks, 0);
    assert!(edited.versions.bytes >= 6 * notes as u64 * size_of::<echode_model::Note>() as u64);
    assert_eq!(edited.journal_changes, 6);

    // Moving `current` changes which version is live, not what the chain holds.
    let total = |session: &Session| {
        let mut sum = session.usage().versions;
        sum += session.project().footprint();
        sum
    };
    let whole = total(&session);
    for _ in 0..4 {
        session.undo().unwrap();
        assert_eq!(total(&session), whole);
    }
    let undone = session.usage();
    assert_eq!((undone.undo_steps, undone.redo_steps), (2, 4));
    assert_eq!(undone.journal_changes, edited.journal_changes);
    session.redo().unwrap();
    assert_eq!(total(&session), whole);

    // A new edit drops the redo branch, and its versions leave the count.
    set_velocity(&mut session, 10, 99);
    let branched = session.usage();
    assert_eq!((branched.undo_steps, branched.redo_steps), (4, 0));
    assert_eq!(branched.versions.contents, 4);
}

#[test]
fn usage_of_a_track_edit_keeps_the_track_and_shares_the_notes() {
    let mut session = session_with_notes(32);
    session
        .perform(Command::AddTrack { name: "b".into() })
        .unwrap();
    // The first parameter of the first track's mixer is its gain.
    let gain = session.project().tracks()[0].devices[0].parameters[0].id;
    session
        .perform(Command::SetParameter {
            parameter: gain,
            value: 0.5,
        })
        .unwrap();
    let usage = session.usage();
    assert_eq!(usage.undo_steps, 2);
    assert_eq!(usage.versions.tracks, 1);
    assert_eq!((usage.versions.contents, usage.versions.notes), (0, 0));
}
