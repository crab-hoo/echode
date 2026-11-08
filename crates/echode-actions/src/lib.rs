//! Actions that change a project, plus undo and a replayable journal.
//!
//! A [`Command`] is what the UI asks for. Performing it records an [`Action`]
//! that already contains every id it created, and the [`ChangeSet`] that
//! resulted. Undo keeps the project from before that action. Recovery applies
//! the changeset when the journal line has one, and replays the actions when
//! it does not.

mod action;
mod journal;
mod session;

pub use action::{Action, ActionError, ActionSpec, Command, catalog, parameter_message};
pub use journal::{JournalEntry, JournalLine, JournalOp, decode, decode_line, encode, encode_line};
pub use session::{HistoryUsage, ParkedRedo, Session, Transaction, replay};
