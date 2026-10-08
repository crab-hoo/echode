//! Actions that change a project, plus undo and a replayable journal.
//!
//! A [`Command`] is what the UI asks for. Performing it records an [`Action`]
//! that already contains every id it created. Undo keeps the project from
//! before that action. The journal is the list of actions that rebuild the
//! current project from the last saved snapshot.

mod action;
mod journal;
mod session;

pub use action::{Action, ActionError, ActionSpec, Command, catalog};
pub use journal::{decode, encode};
pub use session::{Session, Transaction, replay};
