//! A submission returns before the document thread finishes the command.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::Duration;

use echode_audio_io::{AudioCommand, Audition, AuditionOrigin, Session, TransportCancellation};
use echode_history::DocumentHost;

use crate::activity::ActivityHub;
use crate::import::{Stage, product_stage};
use crate::lane::DeviceLane;
use crate::live::LiveState;
use crate::worker::{self, Job, Start};
use crate::{DocumentError, Operation, Presenter, Ticket};

/// Runs on the worker once its session exists.
pub type Configure = Box<dyn FnOnce(&mut Session) + Send>;

/// An open package. The worker, not the caller, runs each operation.
pub struct DocumentService<V> {
    queue: Mutex<Option<Sender<Job<V>>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// A completion for a device request runs here. It cannot join the worker,
    /// which joins this thread on its way out.
    lane_thread: Option<ThreadId>,
    live: Arc<LiveState>,
    cancellation: TransportCancellation,
    activity: Arc<ActivityHub>,
    /// Sample audition requests go straight to their own thread.
    audition: Audition,
}

impl<V: Send + 'static> DocumentService<V> {
    pub fn open<P: Presenter<View = V>>(path: &Path, presenter: P) -> Result<Self, DocumentError> {
        Self::open_configured(path, presenter, Box::new(|_| {}))
    }

    /// Like [`Self::open`], configuring the session on the worker. Tests use
    /// this for a device-free output. Everything else is the product's.
    pub fn open_configured<P: Presenter<View = V>>(
        path: &Path,
        presenter: P,
        configure: Configure,
    ) -> Result<Self, DocumentError> {
        Self::open_staging(path, presenter, configure, product_stage())
    }

    /// Like [`Self::open_configured`], staging imports with `stage`. Tests
    /// hold the product's staging with this.
    pub(crate) fn open_staging<P: Presenter<View = V>>(
        path: &Path,
        presenter: P,
        configure: Configure,
        stage: Stage,
    ) -> Result<Self, DocumentError> {
        let host = DocumentHost::open(path)?;
        let (sender, incoming) = mpsc::channel();
        let live = Arc::new(LiveState::default());
        let cancellation = TransportCancellation::default();
        let lane = DeviceLane::start(P::with_device_report)?;
        let lane_thread = lane.thread_id();
        let activity = Arc::new(ActivityHub::new());
        let audition = Audition::start()?;
        let start = Start {
            host,
            presenter,
            lane,
            live: Arc::clone(&live),
            activity: Arc::clone(&activity),
            cancellation: cancellation.clone(),
            audition: audition.clone(),
            configure,
            stage,
        };
        let thread = thread::Builder::new()
            .name("echode-worker".to_string())
            .spawn(move || worker::run(start, incoming))
            .map_err(DocumentError::Thread)?;
        Ok(Self {
            queue: Mutex::new(Some(sender)),
            thread: Mutex::new(Some(thread)),
            lane_thread,
            live,
            cancellation,
            activity,
            audition,
        })
    }
}

impl<V> DocumentService<V> {
    /// Queues `operation`. `ticket` completes once, here if the document has
    /// stopped, and without any lock held.
    pub fn submit(&self, operation: Operation, ticket: Ticket<V>) {
        let mut job = Job {
            operation,
            ticket,
            transport_revision: 0,
        };
        let rejected = {
            let queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match queue.as_ref() {
                None => Some(job),
                Some(sender) => {
                    // Capture the generation in queue order. Stop silences an
                    // existing callback and invalidates a loading Play here,
                    // without waiting behind that load on the document worker.
                    if let Operation::Audio(bytes) = &job.operation
                        && let Ok(command) = serde_json::from_slice::<AudioCommand>(bytes)
                    {
                        job.transport_revision = self.cancellation.request(&command);
                    }
                    sender.send(job).err().map(|error| error.0)
                }
            }
        };
        drop(rejected);
    }

    /// Plays `path` once over the project, replacing any audition, or fades
    /// the audition out for `None`. Never waits for the document worker.
    /// `origin` is only recorded in the audition's history.
    pub fn audition(&self, path: Option<PathBuf>, origin: AuditionOrigin, start_frame: u64) -> u64 {
        match path {
            Some(path) => self.audition.play_from(path, start_frame, origin),
            None => self.audition.stop_with(origin),
        }
    }

    pub fn configure_audition(&self, visible: bool, gain: f32) -> bool {
        self.audition.configure(visible, gain).is_ok()
    }

    /// Plugin activity after `after`, waiting up to `timeout`. Never waits for
    /// the worker.
    pub fn plugin_activity(&self, after: u64, timeout: Duration) -> Vec<u8> {
        self.activity.read(after, timeout)
    }

    pub fn track_peak(&self, id: u128) -> f32 {
        self.live.peak(id)
    }

    /// The live tick and whether it is advancing. Safe on a draw thread: this
    /// takes no lock and never waits for the worker.
    pub fn position(&self) -> (i64, bool) {
        self.live.position()
    }

    /// Finishes queued commands, then refuses later ones. A completion may call
    /// this: its thread cannot wait for itself, so the owner joins afterwards.
    pub fn shutdown(&self) {
        self.cancellation.request(&AudioCommand::Stop);
        // Cancels a decode in progress. The thread closes its output and exits
        // once the last handle is dropped with this document.
        self.audition.stop();
        self.audition.hide();
        {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            queue.take();
        }
        let handle = {
            let mut thread = self
                .thread
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let current = thread::current().id();
            let on_worker = thread
                .as_ref()
                .is_some_and(|thread| thread.thread().id() == current);
            if on_worker || self.lane_thread == Some(current) {
                return;
            }
            thread.take()
        };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

impl<V> Drop for DocumentService<V> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
