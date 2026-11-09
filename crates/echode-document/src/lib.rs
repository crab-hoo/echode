//! The turns of one open document.
//!
//! A worker thread owns the history, the audio session and the plugin saves.
//! Edits, transport and captured plugin bytes take their turns there, and a
//! device request waits on its own lane so a slow plugin holds up nothing else.
//! Callers submit an [`Operation`] with a [`Ticket`] and never wait for the
//! turn. A [`Presenter`] decides what a finished turn hands back, so this crate
//! knows nothing about the C ABI or what the app draws.

mod activity;
mod device;
mod error;
mod lane;
mod live;
mod saves;
mod service;
mod ticket;
mod worker;

use std::path::Path;
use std::sync::Arc;

use echode_model::Project;

pub use error::{DocumentError, Rejected};
pub use service::{Configure, DocumentService};
pub use ticket::Ticket;

/// What a device report holds when the turn was not a device request.
pub const EMPTY_DEVICE_REPORT: &[u8] = b"{}";
/// What a history report holds when the turn listed and changed nothing.
pub const EMPTY_HISTORY_REPORT: &[u8] = br#"{"revisions":[],"changed":false}"#;

pub enum Operation {
    /// One command, or an array of actions as a single Undo step.
    Perform(Vec<u8>),
    /// A session command. Neither Undo nor the journal records it.
    Audio(Vec<u8>),
    /// History, recovery consent, export snapshots or GUI export preferences.
    History(Vec<u8>),
    /// A request a running device answers. See [`device`].
    Device(Vec<u8>),
    Snapshot,
    Undo,
    Redo,
    /// A request no entry point recognized. It completes with an error.
    Invalid,
}

/// What one finished turn reports, in order with every other turn.
pub struct Publication<'a> {
    pub project: Arc<Project>,
    pub directory: &'a Path,
    /// JSON of the session state.
    pub audio: Vec<u8>,
    /// JSON `{revisions, changed, recovery?, export_settings?}`.
    pub history: Vec<u8>,
    /// JSON report of a device request, or [`EMPTY_DEVICE_REPORT`].
    pub device: Vec<u8>,
}

/// Builds what a completion carries. It runs on the worker, so it may keep and
/// reuse what it built for the previous turn.
pub trait Presenter: Send + 'static {
    type View: Send + 'static;

    fn present(&mut self, publication: Publication<'_>) -> Self::View;

    /// The same view, carrying the report of a device request. This runs on
    /// the device lane, after the helper answered.
    fn with_device_report(view: Self::View, report: Vec<u8>) -> Self::View;
}
