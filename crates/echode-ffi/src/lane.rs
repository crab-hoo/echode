//! Helper round trips for device views. They run here instead of on the
//! document worker, so a plugin that is slow to answer delays only its own
//! requests. Edits, transport and status keep their order on the worker.
//!
//! Requests run one at a time in submission order. A value change therefore
//! reaches its helper before a later capture of that helper's bytes.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use echode_model::Id;

use super::{ECHODE_OK, Job, Snapshot, finish, finish_raw};

/// A blocking exchange with one helper, prepared on the worker.
pub(super) type RoundTrip = Box<dyn FnOnce() -> Result<Vec<u8>, String> + Send>;

enum Work {
    /// Completes the caller with `snapshot`, carrying the reply as its report.
    Report {
        job: Job,
        snapshot: Snapshot,
        round_trip: RoundTrip,
    },
    /// Hands the reply back to the worker, which records it.
    Return {
        job: Job,
        device: Id,
        round_trip: RoundTrip,
    },
}

/// A reply the worker still has to record before completing `job`.
pub(super) struct Returned {
    pub(super) job: Job,
    pub(super) device: Id,
    pub(super) reply: Result<Vec<u8>, String>,
}

pub(super) struct DeviceLane {
    jobs: Option<Sender<Work>>,
    returns: Receiver<Returned>,
    closing: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Returns sent to the lane and not yet taken by the worker.
    waiting: usize,
}

const STOPPED: &str = "document thread stopped";

impl DeviceLane {
    pub(super) fn start() -> Result<Self, String> {
        let (jobs, incoming) = mpsc::channel();
        let (returning, returns) = mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&closing);
        let thread = thread::Builder::new()
            .name("echode-device".to_string())
            .spawn(move || run(incoming, returning, stop))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            jobs: Some(jobs),
            returns,
            closing,
            thread: Some(thread),
            waiting: 0,
        })
    }

    pub(super) fn report(&mut self, job: Job, snapshot: Snapshot, round_trip: RoundTrip) {
        self.send(Work::Report {
            job,
            snapshot,
            round_trip,
        });
    }

    pub(super) fn capture(&mut self, job: Job, device: Id, round_trip: RoundTrip) {
        if self.send(Work::Return {
            job,
            device,
            round_trip,
        }) {
            self.waiting += 1;
        }
    }

    /// True while a reply for the worker is outstanding. The worker then
    /// polls more often so a commit does not wait for its idle timeout.
    pub(super) fn waiting(&self) -> bool {
        self.waiting > 0
    }

    pub(super) fn take_returned(&mut self) -> Option<Returned> {
        let returned = self.returns.try_recv().ok()?;
        self.waiting = self.waiting.saturating_sub(1);
        Some(returned)
    }

    /// Refuses queued work. A round trip already waiting ends when its helper
    /// answers, times out, or exits with the session.
    pub(super) fn close(&mut self) {
        self.closing.store(true, Ordering::Release);
        self.jobs.take();
    }

    /// Waits for the lane, then refuses replies no worker will record.
    pub(super) fn join(&mut self) {
        self.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        while let Some(returned) = self.take_returned() {
            finish(&returned.job, Err(STOPPED.to_string()));
        }
    }

    fn send(&mut self, work: Work) -> bool {
        let rejected = match &self.jobs {
            Some(jobs) => jobs.send(work).err().map(|error| error.0),
            None => Some(work),
        };
        let Some(work) = rejected else {
            return true;
        };
        match work {
            Work::Report { job, .. } | Work::Return { job, .. } => {
                finish(&job, Err(STOPPED.to_string()));
            }
        }
        false
    }
}

impl Drop for DeviceLane {
    fn drop(&mut self) {
        self.join();
    }
}

fn run(incoming: Receiver<Work>, returning: Sender<Returned>, closing: Arc<AtomicBool>) {
    let attempt = |round_trip: RoundTrip| {
        if closing.load(Ordering::Acquire) {
            return Err(STOPPED.to_string());
        }
        catch_unwind(AssertUnwindSafe(round_trip))
            .unwrap_or_else(|_| Err("device request panicked".to_string()))
    };
    for work in incoming {
        match work {
            Work::Report {
                job,
                snapshot,
                round_trip,
            } => match attempt(round_trip) {
                Ok(report) => {
                    let snapshot = Box::into_raw(Box::new(snapshot.with_device(report)));
                    finish_raw(&job, ECHODE_OK, None, snapshot);
                }
                Err(error) => finish(&job, Err(error)),
            },
            Work::Return {
                job,
                device,
                round_trip,
            } => {
                let reply = attempt(round_trip);
                if let Err(error) = returning.send(Returned { job, device, reply }) {
                    finish(&error.0.job, Err(STOPPED.to_string()));
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "lane_tests.rs"]
mod tests;
