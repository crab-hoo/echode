//! Undo stack and transactions.
//!
//! The undo stack keeps a project snapshot per committed group. There is no
//! fixed depth: memory is the only limit, and [`Session::usage`] reports what
//! history holds (ADR 0286). Unchanged tracks stay shared through the `Arc`
//! inside `Project`.

use std::mem::size_of;

use echode_model::{ChangeSet, Footprint, Project};
use serde::Serialize;

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

/// What undo and redo keep in memory beyond the current project (FCA-19).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HistoryUsage {
    pub undo_steps: u64,
    pub redo_steps: u64,
    /// Tracks, contents and notes that only undo and redo versions hold.
    pub versions: Footprint,
    /// Change records the in-memory journal keeps for undo and redo.
    pub journal_changes: u64,
    /// Estimate of those journal entries; text and ID lists are not counted.
    pub journal_bytes: u64,
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

    /// Estimates what undo and redo keep in memory.
    ///
    /// Each version counts what it does not share with its neighbor toward
    /// the current project, so an object kept by several versions counts once.
    /// Time is O(steps × (tracks + contents)); call it off the audio thread.
    pub fn usage(&self) -> HistoryUsage {
        let mut versions = Footprint::default();
        let mut newer = &self.current;
        for older in self.undo.iter().rev() {
            versions += older.footprint_beyond(newer);
            newer = older;
        }
        // Redo pushes the newest undone version last, next to `current`.
        let mut older = &self.current;
        for newer in self.redo.iter().rev() {
            versions += newer.footprint_beyond(older);
            older = newer;
        }
        let entries = self.journal.iter().chain(&self.journal_redo);
        let mut journal_changes = 0u64;
        let mut journal_bytes =
            ((self.journal.len() + self.journal_redo.len()) * size_of::<JournalEntry>()) as u64;
        for entry in entries {
            let changes = entry.changes.as_ref();
            journal_changes = journal_changes
                .saturating_add(changes.map_or(0, |changes| changes.changes().len()) as u64);
            let bytes = entry.actions.len() * size_of::<crate::Action>()
                + changes.map_or(0, ChangeSet::estimated_bytes);
            journal_bytes = journal_bytes.saturating_add(bytes as u64);
        }
        HistoryUsage {
            undo_steps: self.undo.len() as u64,
            redo_steps: self.redo.len() as u64,
            versions,
            journal_changes,
            journal_bytes,
        }
    }

    /// A command with an empty changeset succeeds without recording history.
    pub fn perform(&mut self, command: Command) -> Result<Action, ActionError> {
        let action = Action::from_command(command);
        let (next, changes) = action.apply(&self.current)?;
        if changes.is_empty() {
            return Ok(action);
        }
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
    /// A group with no net change succeeds without touching history or redo.
    pub fn transaction<T>(
        &mut self,
        body: impl FnOnce(&mut Transaction<'_>) -> Result<T, ActionError>,
    ) -> Result<T, ActionError> {
        let checkpoint = self.current.clone();
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
                if !changes.is_empty() && self.current != checkpoint {
                    self.undo.push(checkpoint);
                    self.redo.clear();
                    self.journal_redo.clear();
                    self.journal.push(JournalEntry {
                        actions,
                        changes: Some(changes),
                    });
                } else {
                    // Keep the original sharing and redo branch when the group
                    // cancels itself out or every command is already satisfied.
                    self.current = checkpoint;
                }
                Ok(value)
            }
            Err(error) => {
                self.current = checkpoint;
                Err(error)
            }
        }
    }

    pub fn undo(&mut self) -> Result<ChangeSet, ActionError> {
        let Some(previous) = self.undo.pop() else {
            return Err(ActionError::NothingToUndo);
        };
        let Some(group) = self.journal.pop() else {
            self.undo.push(previous);
            return Err(ActionError::NothingToUndo);
        };
        let changes = group.changes.clone().unwrap_or_default();
        self.redo
            .push(std::mem::replace(&mut self.current, previous));
        self.journal_redo.push(group);
        Ok(changes)
    }

    /// Replaces the project with `restored` as one undo step.
    ///
    /// An empty difference does not record a step. The caller loaded `restored`
    /// from history; this method does not read the package.
    pub fn restore(
        &mut self,
        restored: Project,
        revision: String,
    ) -> Result<ChangeSet, ActionError> {
        restored
            .check_invariants()
            .map_err(|_| ActionError::NotApplied)?;
        let changes = self.current.diff(&restored);
        if changes.is_empty() {
            return Ok(changes);
        }
        debug_assert_eq!(
            self.current.apply_changeset(&changes).ok().as_ref(),
            Some(&restored)
        );
        self.undo
            .push(std::mem::replace(&mut self.current, restored));
        self.redo.clear();
        self.journal_redo.clear();
        self.journal.push(JournalEntry {
            actions: vec![Action::Restore { revision }],
            changes: Some(changes.clone()),
        });
        Ok(changes)
    }

    /// Moves the redo stacks out without copying them.
    ///
    /// A failed publish puts them back. Dropping the value discards the redo,
    /// which is what a new edit does after it is saved.
    pub fn park_redo(&mut self) -> ParkedRedo {
        ParkedRedo {
            redo: std::mem::take(&mut self.redo),
            journal_redo: std::mem::take(&mut self.journal_redo),
        }
    }

    pub fn unpark_redo(&mut self, parked: ParkedRedo) {
        self.redo = parked.redo;
        self.journal_redo = parked.journal_redo;
    }

    /// Removes the edit just committed and restores `parked`.
    pub fn revert_commit(&mut self, parked: ParkedRedo) -> Result<(), ActionError> {
        let Some(previous) = self.undo.pop() else {
            self.unpark_redo(parked);
            return Err(ActionError::NothingToUndo);
        };
        if self.journal.pop().is_none() {
            self.undo.push(previous);
            self.unpark_redo(parked);
            return Err(ActionError::NothingToUndo);
        }
        self.current = previous;
        self.unpark_redo(parked);
        Ok(())
    }

    /// Stores a plugin hash on the current project without an undo step.
    ///
    /// Background capture is not a user edit: redo stays, and the undo depth
    /// does not grow. The returned changeset is what crash recovery replays.
    pub fn absorb_plugin_state(
        &mut self,
        device: echode_model::Id,
        state: Option<String>,
    ) -> Result<Option<ChangeSet>, ActionError> {
        let next = self.current.set_device_state(device, state)?;
        if next == self.current {
            return Ok(None);
        }
        let changes = self.current.diff(&next);
        self.current = next;
        Ok(Some(changes))
    }

    /// Puts `project` back when publishing an absorbed state did not finish.
    pub fn reinstall_current(&mut self, project: Project) {
        self.current = project;
    }

    pub fn redo(&mut self) -> Result<ChangeSet, ActionError> {
        let Some(next) = self.redo.pop() else {
            return Err(ActionError::NothingToRedo);
        };
        let Some(group) = self.journal_redo.pop() else {
            self.redo.push(next);
            return Err(ActionError::NothingToRedo);
        };
        let changes = group.changes.clone().unwrap_or_default();
        self.undo.push(std::mem::replace(&mut self.current, next));
        self.journal.push(group);
        Ok(changes)
    }
}

/// Redo stacks held aside while an edit is published.
pub struct ParkedRedo {
    redo: Vec<Project>,
    journal_redo: Vec<JournalEntry>,
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
        if changes.is_empty() {
            return Ok(action);
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
#[path = "session_tests.rs"]
mod tests;
