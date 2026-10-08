//! Undo stack and transactions.
//!
//! The undo stack keeps a project snapshot per committed group. There is no
//! fixed depth: memory is the only limit. Unchanged tracks stay shared through
//! the `Arc` inside `Project`.

use echode_model::{ChangeSet, Project};

use crate::action::{Action, ActionError, Command};
use crate::journal::JournalEntry;

/// Applies a journal to a saved project. This is the crash-recovery path.
///
/// A line with a changeset is applied as written. A line without one is an
/// older journal, and its commands run through the model again.
pub fn replay(base: &Project, journal: &[JournalEntry]) -> Result<Project, ActionError> {
    let mut project = base.clone();
    for entry in journal {
        project = match &entry.changes {
            Some(changes) => project.apply_changeset(changes)?,
            None => {
                let mut project = project;
                for action in &entry.actions {
                    project = action.apply(&project)?.0;
                }
                project
            }
        };
    }
    Ok(project)
}

/// The editable session: current project, undo, redo, and the journal that
/// rebuilds `current` from the project the session was opened with.
#[derive(Clone, Debug)]
pub struct Session {
    current: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
    journal: Vec<JournalEntry>,
    journal_redo: Vec<JournalEntry>,
}

impl Session {
    pub fn open(project: Project) -> Self {
        Self {
            current: project,
            undo: Vec::new(),
            redo: Vec::new(),
            journal: Vec::new(),
            journal_redo: Vec::new(),
        }
    }

    pub fn project(&self) -> &Project {
        &self.current
    }

    /// Edits that rebuild [`Self::project`] from the project passed to [`Self::open`].
    pub fn journal(&self) -> &[JournalEntry] {
        &self.journal
    }

    pub fn perform(&mut self, command: Command) -> Result<Action, ActionError> {
        let action = Action::from_command(command);
        let (next, changes) = action.apply(&self.current)?;
        self.undo.push(std::mem::replace(&mut self.current, next));
        self.redo.clear();
        self.journal_redo.clear();
        self.journal.push(JournalEntry {
            actions: vec![action.clone()],
            changes: Some(changes),
        });
        Ok(action)
    }

    /// Runs several commands as one undo step. A failure puts the session back.
    pub fn transaction<T>(
        &mut self,
        body: impl FnOnce(&mut Transaction<'_>) -> Result<T, ActionError>,
    ) -> Result<T, ActionError> {
        let checkpoint = self.current.clone();
        let undo_len = self.undo.len();
        let redo = self.redo.clone();
        let journal_redo = self.journal_redo.clone();
        let mut actions = Vec::new();
        let mut changes = ChangeSet::new();
        let result = {
            let mut transaction = Transaction {
                session: self,
                actions: &mut actions,
                changes: &mut changes,
            };
            body(&mut transaction)
        };
        match result {
            Ok(value) => {
                if !actions.is_empty() {
                    self.journal.push(JournalEntry {
                        actions,
                        changes: Some(changes),
                    });
                }
                Ok(value)
            }
            Err(error) => {
                self.current = checkpoint;
                self.undo.truncate(undo_len);
                self.redo = redo;
                self.journal_redo = journal_redo;
                Err(error)
            }
        }
    }

    pub fn undo(&mut self) -> Result<(), ActionError> {
        let Some(previous) = self.undo.pop() else {
            return Err(ActionError::NothingToUndo);
        };
        let Some(group) = self.journal.pop() else {
            self.undo.push(previous);
            return Err(ActionError::NothingToUndo);
        };
        self.redo
            .push(std::mem::replace(&mut self.current, previous));
        self.journal_redo.push(group);
        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), ActionError> {
        let Some(next) = self.redo.pop() else {
            return Err(ActionError::NothingToRedo);
        };
        let Some(group) = self.journal_redo.pop() else {
            self.redo.push(next);
            return Err(ActionError::NothingToRedo);
        };
        self.undo.push(std::mem::replace(&mut self.current, next));
        self.journal.push(group);
        Ok(())
    }
}

/// Borrows a session for the duration of [`Session::transaction`].
pub struct Transaction<'a> {
    session: &'a mut Session,
    actions: &'a mut Vec<Action>,
    changes: &'a mut ChangeSet,
}

impl Transaction<'_> {
    pub fn perform(&mut self, command: Command) -> Result<Action, ActionError> {
        let action = Action::from_command(command);
        let (next, changes) = action.apply(&self.session.current)?;
        if self.actions.is_empty() {
            self.session.undo.push(self.session.current.clone());
            self.session.redo.clear();
            self.session.journal_redo.clear();
        }
        self.session.current = next;
        self.changes.append(changes);
        self.actions.push(action.clone());
        Ok(action)
    }

    pub fn project(&self) -> &Project {
        &self.session.current
    }
}

#[cfg(test)]
mod tests {
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
}
