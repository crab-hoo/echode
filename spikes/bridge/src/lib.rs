//! UI-2 bridge verification, never used by the product.

mod c_api;
#[cfg(feature = "sanitizer-control")]
#[path = "../sanitizers/probe.rs"]
mod sanitizer_control;

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

use echode_model::Project;
use futures_channel::oneshot;

uniffi::setup_scaffolding!();

/// A representative immutable drawing view, prepared once in Rust.
/// It is deliberately independent of UI-1's future visibility query.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BridgeNote {
    pub start: i64,
    pub length: i64,
    pub pitch: u32,
    pub velocity: u32,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct BridgeChange {
    pub revision: u64,
    pub track_count: u64,
}

#[derive(Debug, uniffi::Error)]
pub enum BridgeError {
    EmptyName,
    Stopped,
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyName => "track name is empty",
            Self::Stopped => "document worker stopped",
        })
    }
}

impl std::error::Error for BridgeError {}

#[uniffi::export(callback_interface)]
pub trait ChangeObserver: Send + Sync {
    fn changed(&self, change: BridgeChange);
}

#[cfg(feature = "async-callback")]
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait AsyncChangeObserver: Send + Sync {
    async fn changed(&self, change: BridgeChange);
}

#[cfg(feature = "async-callback")]
#[uniffi::export]
pub async fn notify_async(observer: Box<dyn AsyncChangeObserver>) {
    observer
        .changed(BridgeChange {
            revision: 42,
            track_count: 1,
        })
        .await;
}

#[derive(uniffi::Object)]
pub struct BridgeSnapshot {
    revision: u64,
    project: Arc<Project>,
    notes: Arc<[BridgeNote]>,
    // A per-document counter avoids interference between parallel tests.
    live: Arc<LiveState>,
}

#[derive(Default)]
struct LiveState {
    snapshots: AtomicUsize,
    snapshot_handles: AtomicUsize,
    documents: AtomicUsize,
    workers: AtomicUsize,
    completions: AtomicUsize,
    finished: Mutex<bool>,
    wake: Condvar,
}

impl LiveState {
    #[cfg(any(test, feature = "fault-injection"))]
    fn wait_for_worker(&self) {
        let mut finished = self.finished.lock().unwrap();
        while !*finished {
            finished = self.wake.wait(finished).unwrap();
        }
    }
}

struct WorkerLifetime(Arc<LiveState>);

impl Drop for WorkerLifetime {
    fn drop(&mut self) {
        self.0.workers.fetch_sub(1, Ordering::Relaxed);
        *self.0.finished.lock().unwrap() = true;
        self.0.wake.notify_all();
    }
}

#[cfg(feature = "fault-injection")]
#[derive(Default)]
struct GateState {
    paused: bool,
    entered: bool,
    fail: bool,
}

#[cfg(feature = "fault-injection")]
#[derive(Default)]
struct ProbeGate {
    state: Mutex<GateState>,
    wake: Condvar,
}

#[cfg(feature = "fault-injection")]
impl ProbeGate {
    fn enter(&self) {
        let mut state = self.state.lock().unwrap();
        state.entered = true;
        self.wake.notify_all();
        while state.paused {
            state = self.wake.wait(state).unwrap();
        }
        let fail = std::mem::take(&mut state.fail);
        // Faults happen before mutation and outside locks. Never poison the gate.
        drop(state);
        assert!(!fail, "injected bridge worker failure");
    }
}

impl BridgeSnapshot {
    fn new(
        revision: u64,
        project: Arc<Project>,
        notes: Arc<[BridgeNote]>,
        live: Arc<LiveState>,
    ) -> Arc<Self> {
        live.snapshots.fetch_add(1, Ordering::Relaxed);
        Arc::new(Self {
            revision,
            project,
            notes,
            live,
        })
    }
}

#[uniffi::export]
impl BridgeSnapshot {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn track_count(&self) -> u64 {
        self.project.tracks().len() as u64
    }

    pub fn note_count(&self) -> u64 {
        self.notes.len() as u64
    }

    /// Control probe: UniFFI serializes this sequence into a new Swift array.
    pub fn copied_notes(&self) -> Vec<BridgeNote> {
        self.notes.to_vec()
    }

    /// Tests ownership while keeping both Arcs alive, never integer addresses.
    pub fn shares_project(&self, other: Arc<Self>) -> bool {
        Arc::ptr_eq(&self.project, &other.project)
    }
}

impl Drop for BridgeSnapshot {
    fn drop(&mut self) {
        self.live.snapshots.fetch_sub(1, Ordering::Relaxed);
    }
}

type Reply = Box<dyn FnOnce(Result<Arc<BridgeSnapshot>, BridgeError>) + Send>;

struct Request {
    name: String,
    reply: Reply,
}

#[derive(uniffi::Object)]
pub struct BridgeDocument {
    requests: Arc<Mutex<Option<mpsc::Sender<Request>>>>,
    published: Arc<Mutex<Arc<BridgeSnapshot>>>,
    live: Arc<LiveState>,
    #[cfg(feature = "fault-injection")]
    gate: Arc<ProbeGate>,
}

impl Drop for BridgeDocument {
    fn drop(&mut self) {
        self.close_queue();
        self.live.documents.fetch_sub(1, Ordering::Relaxed);
    }
}

#[uniffi::export]
impl BridgeDocument {
    #[uniffi::constructor]
    pub fn new(note_count: u32) -> Arc<Self> {
        let live = Arc::new(LiveState::default());
        let notes: Arc<[BridgeNote]> = (0..note_count)
            .map(|index| BridgeNote {
                start: i64::from(index) * 960,
                length: 480,
                pitch: index % 128,
                velocity: 32_768,
            })
            .collect();
        let initial = BridgeSnapshot::new(0, Arc::new(Project::new()), notes, Arc::clone(&live));
        let published = Arc::new(Mutex::new(initial));
        let worker_snapshot = Arc::clone(&published);
        let (requests, incoming) = mpsc::channel::<Request>();
        let requests = Arc::new(Mutex::new(Some(requests)));
        let worker_requests = Arc::clone(&requests);
        #[cfg(feature = "fault-injection")]
        let gate = Arc::new(ProbeGate::default());
        #[cfg(feature = "fault-injection")]
        let worker_gate = Arc::clone(&gate);
        live.workers.fetch_add(1, Ordering::Relaxed);
        let worker_lifetime = WorkerLifetime(Arc::clone(&live));
        // This is a control thread, with the same single-owner shape as DocumentHost.
        // Dropping the last sender closes it; no callback has to join its own thread.
        std::thread::Builder::new()
            .name("echode-bridge-probe".into())
            .spawn(move || {
                // Drop published storage before announcing worker exit.
                let _lifetime = worker_lifetime;
                let worker_snapshot = worker_snapshot;
                while let Ok(request) = incoming.recv() {
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        #[cfg(feature = "fault-injection")]
                        worker_gate.enter();
                        if request.name.is_empty() {
                            Err(BridgeError::EmptyName)
                        } else {
                            let old = Arc::clone(&worker_snapshot.lock().unwrap());
                            let (project, _) = old.project.add_track(request.name);
                            project.check_invariants().unwrap();
                            let next = BridgeSnapshot::new(
                                old.revision + 1,
                                Arc::new(project),
                                Arc::clone(&old.notes),
                                Arc::clone(&old.live),
                            );
                            *worker_snapshot.lock().unwrap() = Arc::clone(&next);
                            Ok(next)
                        }
                    }));
                    let failed = result.is_err();
                    if failed {
                        // Close admission before callbacks; drain accepted work with errors.
                        // No queue or snapshot lock is held across foreign code.
                        worker_requests.lock().unwrap().take();
                    }
                    // No lock is held across foreign code, which may read a snapshot.
                    (request.reply)(result.unwrap_or(Err(BridgeError::Stopped)));
                    if failed {
                        for pending in incoming {
                            (pending.reply)(Err(BridgeError::Stopped));
                        }
                        break;
                    }
                }
            })
            .expect("document probe thread");
        live.documents.fetch_add(1, Ordering::Relaxed);
        Arc::new(Self {
            requests,
            published,
            live,
            #[cfg(feature = "fault-injection")]
            gate,
        })
    }

    pub fn snapshot(&self) -> Arc<BridgeSnapshot> {
        Arc::clone(&self.published.lock().unwrap())
    }

    pub fn live_snapshots(&self) -> u64 {
        self.live.snapshots.load(Ordering::Relaxed) as u64
    }

    pub async fn add_track(
        &self,
        name: String,
        observer: Box<dyn ChangeObserver>,
    ) -> Result<Arc<BridgeSnapshot>, BridgeError> {
        let (reply, incoming) = oneshot::channel();
        self.enqueue(
            name,
            Box::new(move |result| {
                if let Ok(snapshot) = &result {
                    observer.changed(BridgeChange {
                        revision: snapshot.revision(),
                        track_count: snapshot.track_count(),
                    });
                }
                let _ = reply.send(result);
            }),
        );
        incoming.await.map_err(|_| BridgeError::Stopped)?
    }
}

impl BridgeDocument {
    fn close_queue(&self) {
        self.requests.lock().unwrap().take();
    }

    fn enqueue(&self, name: String, reply: Reply) {
        let request = Request { name, reply };
        let refused = {
            let requests = self.requests.lock().unwrap();
            if let Some(sender) = &*requests {
                sender.send(request).err().map(|error| error.0)
            } else {
                Some(request)
            }
        };
        if let Some(request) = refused {
            (request.reply)(Err(BridgeError::Stopped));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    #[test]
    fn audit_ui2_snapshot_handles_share_and_release_storage() {
        let document = BridgeDocument::new(100_000);
        let old = document.snapshot();
        let same = document.snapshot();
        assert!(old.shares_project(Arc::clone(&same)));
        let (reply, incoming) = mpsc::channel();
        document.enqueue(
            "piano".into(),
            Box::new(move |result| reply.send(result).unwrap()),
        );
        let new = incoming.recv().unwrap().unwrap();
        assert_eq!(old.track_count(), 0);
        assert_eq!(new.track_count(), 1);
        assert!(!old.shares_project(Arc::clone(&new)));
        assert!(Arc::ptr_eq(&old.notes, &new.notes));
        assert_eq!(document.live_snapshots(), 2);
        drop(old);
        drop(same);
        assert_eq!(document.live_snapshots(), 1);
    }

    #[test]
    fn rejected_command_does_not_publish_a_snapshot() {
        let document = BridgeDocument::new(0);
        let before = document.snapshot();
        let (reply, incoming) = mpsc::channel();
        document.enqueue(
            String::new(),
            Box::new(move |result| reply.send(result).unwrap()),
        );
        assert!(matches!(
            incoming.recv().unwrap(),
            Err(BridgeError::EmptyName)
        ));
        assert!(before.shares_project(document.snapshot()));
        assert_eq!(document.live_snapshots(), 1);
    }

    struct Observer(mpsc::Sender<u64>);

    impl ChangeObserver for Observer {
        fn changed(&self, change: BridgeChange) {
            self.0.send(change.revision).unwrap();
        }
    }

    #[test]
    fn dropping_an_enqueued_future_keeps_the_edit_and_notification() {
        let document = BridgeDocument::new(0);
        let (entered, entering) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        document.enqueue(
            "first".into(),
            Box::new(move |_| {
                entered.send(()).unwrap();
                gate.recv().unwrap();
            }),
        );
        entering.recv().unwrap();
        let (changed, changes) = mpsc::channel();
        let mut future = Box::pin(document.add_track("second".into(), Box::new(Observer(changed))));
        let waker = Waker::noop();
        assert!(matches!(
            future.as_mut().poll(&mut Context::from_waker(waker)),
            Poll::Pending
        ));
        drop(future);
        release.send(()).unwrap();
        assert_eq!(changes.recv().unwrap(), 2);
        assert_eq!(document.snapshot().track_count(), 2);
    }

    #[test]
    fn a_stopped_queue_completes_instead_of_leaking_a_reply() {
        let document = BridgeDocument::new(0);
        document.close_queue();
        let (reply, incoming) = mpsc::channel();
        document.enqueue(
            "piano".into(),
            Box::new(move |result| reply.send(result).unwrap()),
        );
        assert!(matches!(
            incoming.recv().unwrap(),
            Err(BridgeError::Stopped)
        ));
    }
}
