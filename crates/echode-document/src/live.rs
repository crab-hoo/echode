//! What the app reads without waiting for the worker: the playhead, from a
//! draw thread without a lock, and each track's meter.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use echode_audio_io::TrackMeter;

#[derive(Default)]
pub(crate) struct LiveState {
    meters: RwLock<BTreeMap<u128, Arc<TrackMeter>>>,
    generation: AtomicU32,
    tick: AtomicI64,
    playing: AtomicU32,
}

impl LiveState {
    pub(crate) fn peak(&self, track: u128) -> f32 {
        self.meters
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&track)
            .map_or(0.0, |meter| meter.peak())
    }

    /// A coherent copy of the tick and whether it is advancing.
    pub(crate) fn position(&self) -> (i64, bool) {
        loop {
            let generation = self.generation.load(Ordering::SeqCst);
            if !generation.is_multiple_of(2) {
                continue;
            }
            let position = (
                self.tick.load(Ordering::SeqCst),
                self.playing.load(Ordering::SeqCst) != 0,
            );
            if generation == self.generation.load(Ordering::SeqCst) {
                return position;
            }
        }
    }

    pub(crate) fn set_position(&self, tick: i64, playing: bool) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.tick.store(tick, Ordering::SeqCst);
        self.playing.store(u32::from(playing), Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn set_meters(&self, meters: Vec<Arc<TrackMeter>>) {
        let mut published = self
            .meters
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if published.len() != meters.len()
            || meters.iter().any(|meter| {
                published
                    .get(&meter.track)
                    .is_none_or(|old| !Arc::ptr_eq(old, meter))
            })
        {
            *published = meters
                .into_iter()
                .map(|meter| (meter.track, meter))
                .collect();
        }
    }

    /// Nothing sounds once the session has ended.
    pub(crate) fn clear(&self) {
        self.meters
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.playing.store(0, Ordering::SeqCst);
    }
}
