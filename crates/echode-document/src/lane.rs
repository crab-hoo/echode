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
use std::thread::{self, JoinHandle, ThreadId};

use echode_model::Id;

use crate::ticket::{STOPPED, Ticket};

/// A blocking exchange with one helper, prepared on the worker.
pub(crate) type RoundTrip = Box<dyn FnOnce() -> Result<Vec<u8>, String> + Send>;

/// Puts a device report on the view a request completes with.
pub(crate) type AttachReport<V> = fn(V, Vec<u8>) -> V;

enum Work<V> {
    /// Completes the caller with `view`, carrying the reply as its report.
    Report {
        ticket: Ticket<V>,
        view: V,
        round_trip: RoundTrip,
    },
    /// Hands the reply back to the worker, which records it.
    Return {
        ticket: Ticket<V>,
        device: Id,
        round_trip: RoundTrip,
    },
}

/// A reply the worker still has to record before completing `ticket`.
pub(crate) struct Returned<V> {
    pub(crate) ticket: Ticket<V>,
    pub(crate) device: Id,
    pub(crate) reply: Result<Vec<u8>, String>,
}

pub(crate) struct DeviceLane<V> {
    jobs: Option<Sender<Work<V>>>,
    returns: Receiver<Returned<V>>,
    closing: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Returns sent to the lane and not yet taken by the worker.
    waiting: usize,
}

impl<V: Send + 'static> DeviceLane<V> {
    pub(crate) fn start(attach: AttachReport<V>) -> Result<Self, String> {
        let (jobs, incoming) = mpsc::channel();
        let (returning, returns) = mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&closing);
        let thread = thread::Builder::new()
            .name("echode-device".to_string())
            .spawn(move || run(incoming, returning, stop, attach))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            jobs: Some(jobs),
            returns,
            closing,
            thread: Some(thread),
            waiting: 0,
        })
    }
}

impl<V> DeviceLane<V> {
    /// The thread a completion from this lane runs on, until it is joined.
    pub(crate) fn thread_id(&self) -> Option<ThreadId> {
        self.thread.as_ref().map(|thread| thread.thread().id())
    }

    pub(crate) fn report(&mut self, ticket: Ticket<V>, view: V, round_trip: RoundTrip) {
        self.send(Work::Report {
            ticket,
            view,
            round_trip,
        });
    }

    pub(crate) fn capture(&mut self, ticket: Ticket<V>, device: Id, round_trip: RoundTrip) {
        if self.send(Work::Return {
            ticket,
            device,
            round_trip,
        }) {
            self.waiting += 1;
        }
    }

    /// True while a reply for the worker is outstanding. The worker then
    /// polls more often so a commit does not wait for its idle timeout.
    pub(crate) fn waiting(&self) -> bool {
        self.waiting > 0
    }

    pub(crate) fn take_returned(&mut self) -> Option<Returned<V>> {
        let returned = self.returns.try_recv().ok()?;
        self.waiting = self.waiting.saturating_sub(1);
        Some(returned)
    }

    /// Refuses queued work. A round trip already waiting ends when its helper
    /// answers, times out, or exits with the session.
    pub(crate) fn close(&mut self) {
        self.closing.store(true, Ordering::Release);
        self.jobs.take();
    }

    /// Waits for the lane. Replies no worker will record drop with their
    /// tickets, which refuses them.
    pub(crate) fn join(&mut self) {
        self.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        while self.take_returned().is_some() {}
    }

    /// False when the lane has closed. The work drops with its ticket.
    fn send(&mut self, work: Work<V>) -> bool {
        match &self.jobs {
            Some(jobs) => jobs.send(work).is_ok(),
            None => false,
        }
    }
}

impl<V> Drop for DeviceLane<V> {
    fn drop(&mut self) {
        self.join();
    }
}

fn run<V>(
    incoming: Receiver<Work<V>>,
    returning: Sender<Returned<V>>,
    closing: Arc<AtomicBool>,
    attach: AttachReport<V>,
) {
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
                ticket,
                view,
                round_trip,
            } => ticket.finish(attempt(round_trip).map(|report| attach(view, report))),
            Work::Return {
                ticket,
                device,
                round_trip,
            } => {
                let reply = attempt(round_trip);
                // A worker that has gone refuses it by dropping the ticket.
                let _ = returning.send(Returned {
                    ticket,
                    device,
                    reply,
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "lane_tests.rs"]
mod tests;
