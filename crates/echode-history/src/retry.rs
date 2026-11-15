//! Retry and diagnostic suppression for non-realtime background work.

use std::fmt::Debug;
use std::time::{Duration, Instant};

const FIRST: Duration = Duration::from_millis(100);
const MAX: Duration = Duration::from_secs(5);

/// A failure keeps its work pending. Only success clears its diagnostic and
/// delay; newer input and notifications must not bypass an outstanding delay.
#[derive(Default)]
pub struct BackgroundRetry {
    next: Option<Instant>,
    delay: Option<Duration>,
    failure: Option<String>,
    stopped: bool,
}

impl BackgroundRetry {
    pub fn ready(&self, now: Instant) -> bool {
        !self.stopped && self.next.is_none_or(|next| now >= next)
    }
    /// A failed attempt still needs a retry even without another notification.
    pub fn pending(&self) -> bool {
        self.next.is_some()
    }

    /// Called after an attempt completes. True only for a new failure kind or
    /// changed details, so the caller can log without flooding its sinks.
    pub fn failed(&mut self, now: Instant, error: &impl Debug) -> bool {
        let delay = self.delay.map_or(FIRST, |delay| (delay * 2).min(MAX));
        self.delay = Some(delay);
        self.next = Some(now + delay);
        self.note(error)
    }

    /// A disconnected worker cannot recover within this session. Do not keep
    /// submitting to it, but retain a reportable stopped state.
    pub fn stop(&mut self, error: &impl Debug) -> bool {
        self.stopped = true;
        self.note(error)
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub fn succeeded(&mut self) {
        *self = Self::default();
    }

    fn note(&mut self, error: &impl Debug) -> bool {
        let failure = format!("{error:?}");
        if self.failure.as_ref() == Some(&failure) {
            return false;
        }
        self.failure = Some(failure);
        true
    }
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
