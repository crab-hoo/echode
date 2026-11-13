//! Replacement helper startup never owns the document's control turn (ADR 0142).
use super::{HostGraph, Pending, PlaybackError};
use crate::plugins::{PluginWorkgroup, ReconcileRoutes};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

type GraphResult = (HostGraph, Pending, Result<(), PlaybackError>);

pub(super) struct GraphPreparation {
    thread: Option<JoinHandle<GraphResult>>,
    /// Stops helper starts when this preparation is no longer wanted (ADR 0334).
    cancel: Arc<AtomicBool>,
    pub(super) restore_states: bool,
    pub(super) event_caps: Vec<usize>,
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
        let preserve = hosts.matching_states(&pending.chain, &pending.routes);
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
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
                            cancel: Some(&stop),
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
            cancel,
            restore_states,
            event_caps,
        })
    }

    /// Stops starting helpers. The thread then ends with
    /// [`PlaybackError::Cancelled`] after stopping the ones it started.
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
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
        // Closing an output stops helper starts instead of waiting for them
        // (ADR 0334). Normal service only drops this after poll observed an
        // exited thread, so this cancels nothing then.
        self.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
