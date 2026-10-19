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
