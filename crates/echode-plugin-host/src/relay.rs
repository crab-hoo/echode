//! Parameter changes a plugin reports on its own, from `process` and `flush`.
//!
//! The processing thread writes value and gesture events into a ring that was
//! allocated with the bridge. The control thread reads them, asks the main
//! thread for the values and texts, and sends one relay notice to the parent.
//! A full ring drops events and sets a flag. The parent then reads the list
//! again and treats the change as finished, instead of trusting a gesture
//! whose end may be missing.

use clack_host::events::UnknownEvent;
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use echode_sandbox::{ParamEvent, ParamEventKind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Events kept between two reads of the control thread, which reads at least
/// every 20 ms. This sizes one buffer, not the parameters a plugin may have;
/// a burst beyond it is reported as lost rather than dropped silently.
const RELAY_CAPACITY: usize = 4096;

/// Single producer (the processing thread), single consumer (the control thread).
pub(crate) struct RelayRing {
    slots: Box<[AtomicU64]>,
    /// Next slot the consumer reads.
    head: AtomicUsize,
    /// Next slot the producer writes.
    tail: AtomicUsize,
    lost: AtomicBool,
}

impl RelayRing {
    pub(crate) fn new() -> Self {
        Self {
            slots: (0..RELAY_CAPACITY).map(|_| AtomicU64::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            lost: AtomicBool::new(false),
        }
    }

    /// The slot for the running count `position`. `None` only for no slots.
    fn slot(&self, position: usize) -> Option<&AtomicU64> {
        self.slots.get(position.checked_rem(self.slots.len())?)
    }

    /// Processing thread only. Never allocates, locks or waits.
    pub(crate) fn push(&self, id: u32, kind: ParamEventKind) {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail.wrapping_sub(self.head.load(Ordering::Acquire)) >= self.slots.len() {
            self.lost.store(true, Ordering::Release);
            return;
        }
        let code: u64 = match kind {
            ParamEventKind::Value => 0,
            ParamEventKind::Begin => 1,
            ParamEventKind::End => 2,
        };
        let Some(slot) = self.slot(tail) else {
            self.lost.store(true, Ordering::Release);
            return;
        };
        slot.store(code << 32 | u64::from(id), Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
    }

    /// True when events or a loss wait for the control thread.
    pub(crate) fn pending(&self) -> bool {
        self.tail.load(Ordering::Acquire) != self.head.load(Ordering::Relaxed)
            || self.lost.load(Ordering::Acquire)
    }

    /// Control thread only. Appends the waiting events in order and returns
    /// whether any were dropped since the last read.
    pub(crate) fn drain(&self, into: &mut Vec<ParamEvent>) -> bool {
        let lost = self.lost.swap(false, Ordering::AcqRel);
        let tail = self.tail.load(Ordering::Acquire);
        let mut head = self.head.load(Ordering::Relaxed);
        while head != tail {
            let code = self
                .slot(head)
                .map_or(0, |slot| slot.load(Ordering::Relaxed));
            let kind = match code >> 32 {
                1 => ParamEventKind::Begin,
                2 => ParamEventKind::End,
                _ => ParamEventKind::Value,
            };
            into.push(ParamEvent {
                id: code as u32,
                kind,
            });
            head = head.wrapping_add(1);
        }
        self.head.store(head, Ordering::Release);
        lost
    }
}

/// Output events of one `process` or `flush` call. Parameter values and
/// gestures go to the ring; other output events are not used by the host.
pub(crate) struct RelayOutput<'a>(pub(crate) &'a RelayRing);

impl OutputEventBuffer for RelayOutput<'_> {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let (id, kind) = match event.as_core_event() {
            Some(CoreEventSpace::ParamValue(event)) => (event.param_id(), ParamEventKind::Value),
            Some(CoreEventSpace::ParamGestureBegin(event)) => {
                (event.param_id(), ParamEventKind::Begin)
            }
            Some(CoreEventSpace::ParamGestureEnd(event)) => (event.param_id(), ParamEventKind::End),
            _ => return Ok(()),
        };
        // CLAP's invalid id names no parameter, so there is nothing to relay.
        if let Some(id) = id {
            self.0.push(id.get(), kind);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_order_and_reports_overflow() {
        let ring = RelayRing::new();
        ring.push(1, ParamEventKind::Begin);
        ring.push(1, ParamEventKind::Value);
        ring.push(1, ParamEventKind::End);
        assert!(ring.pending());
        let mut events = Vec::new();
        assert!(!ring.drain(&mut events));
        let kinds: Vec<_> = events.iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            [
                ParamEventKind::Begin,
                ParamEventKind::Value,
                ParamEventKind::End
            ]
        );
        assert!(!ring.pending());
        for _ in 0..RELAY_CAPACITY + 3 {
            ring.push(u32::MAX - 1, ParamEventKind::Value);
        }
        events.clear();
        assert!(ring.drain(&mut events), "the burst beyond the ring is lost");
        assert_eq!(events.len(), RELAY_CAPACITY);
        assert!(events.iter().all(|event| event.id == u32::MAX - 1));
        assert!(!ring.pending());
    }
}
