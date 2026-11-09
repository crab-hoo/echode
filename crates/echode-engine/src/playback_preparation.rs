//! Replacement helper startup never owns the document's control turn (ADR 0142).
use super::{HostGraph, Pending, PlaybackError};
use crate::plugins::{PluginWorkgroup, ReconcileRoutes};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::thread::JoinHandle;

type GraphResult = (HostGraph, Pending, Result<(), PlaybackError>);

pub(super) struct GraphPreparation {
    thread: Option<JoinHandle<GraphResult>>,
    pub(super) restore_states: bool,
    pub(super) event_caps: Vec<usize>,
    pub(super) tail_samples: u32,
}

impl GraphPreparation {
    pub(super) fn begin(
        mut hosts: HostGraph,
        pending: Pending,
        host_bin: PathBuf,
        frames: usize,
        rate: u32,
        workgroup: Option<PluginWorkgroup>,
    ) -> Result<Self, PlaybackError> {
        let restore_states = pending.restore_states;
        let event_caps = hosts.track_event_caps();
        let tail_samples = hosts.tail_samples();
        let preserve = hosts.matching_states(&pending.chain, &pending.routes);
        let thread = std::thread::Builder::new()
            .name("echode-graph-prepare".into())
            .spawn(move || {
                hosts.take_restarts();
                let checkpoint = hosts.preparation_checkpoint();
                let result = catch_unwind(AssertUnwindSafe(|| {
                    hosts.reconcile(
                        &host_bin,
                        ReconcileRoutes {
                            entries: &pending.routes,
                            restore_states: pending.restore_states,
                            preserve: &preserve,
                        },
                        &pending.chain,
                        frames,
                        rate,
                        workgroup.as_ref(),
                    )
                }))
                .unwrap_or(Err(PlaybackError::Plugin));
                let result = result.or_else(|error| {
                    hosts.cancel_preparation(&checkpoint)?;
                    Err(error)
                });
                (hosts, pending, result)
            })
            .map_err(PlaybackError::thread)?;
        Ok(Self {
            thread: Some(thread),
            restore_states,
            event_caps,
            tail_samples,
        })
    }

    /// Joins only an exited thread. No socket, helper or preparation wait here.
    pub(super) fn poll(&mut self) -> Option<Result<GraphResult, PlaybackError>> {
        if !self.thread.as_ref()?.is_finished() {
            return None;
        }
        Some(
            self.thread
                .take()?
                .join()
                .map_err(|_| PlaybackError::Plugin),
        )
    }
}

impl Drop for GraphPreparation {
    fn drop(&mut self) {
        // Closing a document still waits for startup, as the synchronous path did.
        // Normal service only drops this after poll observed an exited thread.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
