//! In-memory project used by the history spike.
//!
//! One thousand tracks is the benchmark population, not a limit of the DAW.
//! Tracks are immutable and reference-counted so an edit clones the changed
//! track only, and scheduling a save does not serialize anything.

use std::sync::Arc;

/// Benchmark population. Not a product limit.
pub const SPIKE_TRACKS: usize = 1_000;
pub const SPIKE_NOTES_PER_TRACK: usize = 100;
pub const TICKS_PER_BEAT: i64 = 705_600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub start: i64,
    pub length: i64,
    pub pitch: u8,
    pub velocity: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub gain_milli_db: i32,
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub tracks: Vec<Arc<Track>>,
}

impl Project {
    pub fn benchmark() -> Arc<Self> {
        let tracks = (0..SPIKE_TRACKS)
            .map(Track::benchmark)
            .map(Arc::new)
            .collect();
        Arc::new(Self { tracks })
    }

    pub fn with_gain(&self, index: usize, gain_milli_db: i32) -> Arc<Self> {
        let mut tracks = self.tracks.clone();
        let current = &tracks[index];
        tracks[index] = Arc::new(Track {
            gain_milli_db,
            ..current.as_ref().clone()
        });
        Arc::new(Self { tracks })
    }
}

impl Track {
    fn benchmark(index: usize) -> Self {
        let notes = (0..SPIKE_NOTES_PER_TRACK)
            .map(|note| Note {
                start: note as i64 * TICKS_PER_BEAT,
                length: TICKS_PER_BEAT,
                pitch: u8::try_from(60 + (note + index) % 24).unwrap_or(60),
                velocity: 100,
            })
            .collect();
        Self {
            id: format!("t{index:04}"),
            name: format!("Track {index:04}"),
            gain_milli_db: 0,
            notes,
        }
    }
}
