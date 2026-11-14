//! The audio thread's part of the log (ADR 0322).
//!
//! The callback never logs: a record allocates and its sinks may wait. It
//! pushes fixed-size [`Note`]s into a ring instead, and counts the ones a full
//! ring refuses. The control thread drains the ring in
//! `PlaybackControl::service`, names each node from its copy of the project,
//! and logs. Automatic mutes are state the callback already publishes, so the
//! control thread logs their changes without a note.
//!
//! A plugin that misses every block must not flood the log. The first note of
//! an episode logs at once; later ones are counted and summarised at most every
//! [`SUMMARY`], and an episode ends after [`SUMMARY`] without a note.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use echode_model::{Id, Project};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::meter::TrackMeter;
use crate::plugins::{MISS_DEADLINE, MISS_EXITED, MISS_PROCESS, MISS_SUBMIT};

const TARGET: &str = "echode_engine::audio";

/// Notes the callback can leave before the control thread drains them. The
/// document thread drains at least every 20 ms; this bounds the diagnostic
/// ring only. A fuller ring drops notes and the log counts them.
const NOTES: usize = 1024;

/// The longest a counted episode waits for its summary, and the quiet time
/// that ends it.
const SUMMARY: Duration = Duration::from_secs(10);

/// What a block reports for the log. Copied into the ring; never allocates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Note {
    /// A plugin stage did not finish this block. `reason` is a `MISS_*` code.
    Plugin {
        device: u128,
        reason: u8,
        sample: i64,
    },
    /// The block returned after the deadline its callback gave it.
    Late {
        sample: i64,
        late_ns: u64,
        frames: u32,
    },
}

/// The callback's end of the ring.
pub(crate) struct NoteSender {
    ring: Producer<Note>,
    dropped: Arc<AtomicU64>,
}

impl NoteSender {
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub(crate) fn send(&mut self, note: Note) {
        if self.ring.push(note).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub(crate) fn note_pair() -> (NoteSender, AudioLog) {
    let (ring, notes) = RingBuffer::new(NOTES);
    let dropped = Arc::new(AtomicU64::new(0));
    (
        NoteSender {
            ring,
            dropped: Arc::clone(&dropped),
        },
        AudioLog {
            notes,
            dropped,
            episodes: BTreeMap::new(),
            muted: BTreeSet::new(),
            master_muted: false,
        },
    )
}

/// What an episode counts: one plugin's misses for one reason, late blocks,
/// or notes the full ring dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Plugin { device: u128, reason: u8 },
    Late,
    Dropped,
}

struct Episode {
    reported: Instant,
    last: Instant,
    /// Notes since the line that `reported` stamps.
    pending: u64,
    first_sample: i64,
    last_sample: i64,
    worst_ns: u64,
}

/// The control thread's end: drains notes and logs them by episode.
pub(crate) struct AudioLog {
    notes: Consumer<Note>,
    dropped: Arc<AtomicU64>,
    episodes: BTreeMap<Key, Episode>,
    /// Tracks the log last saw muted automatically.
    muted: BTreeSet<u128>,
    master_muted: bool,
}

/// What the control thread knows when it drains.
pub(crate) struct Scene<'a> {
    pub(crate) project: &'a Project,
    pub(crate) meters: &'a BTreeMap<u128, Arc<TrackMeter>>,
    /// The master guard's peak and sample when it has muted the output.
    pub(crate) master: Option<(f32, i64)>,
    /// True when the helper behind `device` has exited.
    pub(crate) exited: &'a dyn Fn(u128) -> bool,
}

impl AudioLog {
    pub(crate) fn drain(&mut self, now: Instant, scene: &Scene<'_>) {
        // Quiet episodes are summarised and forgotten first, so a new note
        // starts a fresh one instead of replacing an unreported count.
        self.summarise(now, false, scene);
        while let Ok(note) = self.notes.pop() {
            match note {
                Note::Plugin {
                    device,
                    reason,
                    sample,
                } => {
                    // A deadline miss of a helper that has gone is its exit.
                    let reason = if reason == MISS_DEADLINE && (scene.exited)(device) {
                        MISS_EXITED
                    } else {
                        reason
                    };
                    if self.count(now, Key::Plugin { device, reason }, sample, 0) {
                        let node = Node::of(scene.project, device);
                        log::warn!(
                            target: TARGET,
                            device:% = Id::from_raw(device),
                            track:% = node.track_text(),
                            plugin = node.plugin.unwrap_or("unknown"),
                            reason = reason_name(reason),
                            sample = sample;
                            "a plugin did not finish its audio block"
                        );
                    }
                }
                Note::Late {
                    sample,
                    late_ns,
                    frames,
                } => {
                    if self.count(now, Key::Late, sample, late_ns) {
                        log::warn!(
                            target: TARGET,
                            sample = sample,
                            late_us = late_ns / 1_000,
                            frames = frames;
                            "an audio block finished after its deadline"
                        );
                    }
                }
            }
        }
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped != 0 {
            if self.count(now, Key::Dropped, 0, 0) {
                log::warn!(
                    target: TARGET,
                    dropped = dropped,
                    capacity = NOTES;
                    "the audio thread's log ring was full; later drops are counted"
                );
            } else if let Some(episode) = self.episodes.get_mut(&Key::Dropped) {
                // `count` added this drain as one; the episode counts notes.
                episode.pending = episode.pending.saturating_add(dropped - 1);
            }
        }
        self.mutes(scene);
    }

    /// Logs every count still pending. The owner calls this as it closes.
    pub(crate) fn finish(&mut self, scene: &Scene<'_>) {
        let now = Instant::now();
        self.drain(now, scene);
        self.summarise(now, true, scene);
    }

    /// Counts a note in its episode. True when the note starts an episode,
    /// which the caller then logs in full.
    fn count(&mut self, now: Instant, key: Key, sample: i64, late_ns: u64) -> bool {
        if let Some(episode) = self.episodes.get_mut(&key)
            && now.saturating_duration_since(episode.last) < SUMMARY
        {
            if episode.pending == 0 {
                episode.first_sample = sample;
            }
            episode.last = now;
            episode.pending = episode.pending.saturating_add(1);
            episode.last_sample = sample;
            episode.worst_ns = episode.worst_ns.max(late_ns);
            return false;
        }
        // A quiet episode was summarised and removed before this note came.
        self.episodes.insert(
            key,
            Episode {
                reported: now,
                last: now,
                pending: 0,
                first_sample: sample,
                last_sample: sample,
                worst_ns: 0,
            },
        );
        true
    }

    /// Summarises counted notes every [`SUMMARY`], or all of them at `closing`,
    /// and forgets episodes that have gone quiet.
    fn summarise(&mut self, now: Instant, closing: bool, scene: &Scene<'_>) {
        self.episodes.retain(|key, episode| {
            let quiet = now.saturating_duration_since(episode.last) >= SUMMARY;
            if episode.pending != 0
                && (closing || quiet || now.saturating_duration_since(episode.reported) >= SUMMARY)
            {
                let seconds = now
                    .saturating_duration_since(episode.reported)
                    .as_secs_f64();
                match *key {
                    Key::Plugin { device, reason } => {
                        let node = Node::of(scene.project, device);
                        log::warn!(
                            target: TARGET,
                            device:% = Id::from_raw(device),
                            track:% = node.track_text(),
                            plugin = node.plugin.unwrap_or("unknown"),
                            reason = reason_name(reason),
                            count = episode.pending,
                            seconds = seconds,
                            first_sample = episode.first_sample,
                            last_sample = episode.last_sample;
                            "a plugin kept missing audio blocks"
                        );
                    }
                    Key::Late => log::warn!(
                        target: TARGET,
                        count = episode.pending,
                        seconds = seconds,
                        worst_late_us = episode.worst_ns / 1_000,
                        last_sample = episode.last_sample;
                        "more audio blocks finished after their deadline"
                    ),
                    Key::Dropped => log::warn!(
                        target: TARGET,
                        count = episode.pending,
                        seconds = seconds;
                        "the audio thread's log ring dropped notes"
                    ),
                }
                episode.pending = 0;
                episode.worst_ns = 0;
                episode.reported = now;
            }
            !(closing || quiet)
        });
    }

    /// Logs automatic mutes as they start and end (ADR 0282).
    fn mutes(&mut self, scene: &Scene<'_>) {
        for (track, meter) in scene.meters {
            match (meter.auto_mute(), self.muted.contains(track)) {
                (Some(mute), false) => {
                    if let Some(device) = mute.device {
                        let node = Node::of(scene.project, device);
                        log::warn!(
                            target: TARGET,
                            track:% = Id::from_raw(*track),
                            device:% = Id::from_raw(device),
                            plugin = node.plugin.unwrap_or("unknown"),
                            sample = mute.at;
                            "a track was muted because a plugin returned a non-finite sample"
                        );
                    } else {
                        log::warn!(
                            target: TARGET,
                            track:% = Id::from_raw(*track),
                            sample = mute.at;
                            "a track was muted because its audio sum produced a non-finite sample"
                        );
                    }
                    self.muted.insert(*track);
                }
                (None, true) => {
                    log::info!(target: TARGET, track:% = Id::from_raw(*track); "a track's automatic mute ended");
                    self.muted.remove(track);
                }
                _ => {}
            }
        }
        self.muted.retain(|track| scene.meters.contains_key(track));
        match (scene.master, self.master_muted) {
            (Some((peak, sample)), false) => {
                log::warn!(
                    target: TARGET,
                    peak = f64::from(peak),
                    sample = sample;
                    "the master output was muted after a peak above the guard"
                );
                self.master_muted = true;
            }
            (None, true) => {
                log::info!(target: TARGET, "the master output's automatic mute ended");
                self.master_muted = false;
            }
            _ => {}
        }
    }
}

/// The track and plugin that hold a device, from the control thread's project.
struct Node<'a> {
    track: Option<Id>,
    plugin: Option<&'a str>,
}

impl<'a> Node<'a> {
    fn of(project: &'a Project, device: u128) -> Self {
        project
            .tracks()
            .iter()
            .find_map(|track| {
                let found = track.devices.iter().find(|item| item.id.raw() == device)?;
                Some(Self {
                    track: Some(track.id),
                    plugin: found.plugin.as_deref(),
                })
            })
            .unwrap_or(Self {
                track: None,
                plugin: None,
            })
    }

    fn track_text(&self) -> String {
        self.track
            .map_or_else(|| "unknown".to_owned(), |track| track.to_string())
    }
}

fn reason_name(reason: u8) -> &'static str {
    match reason {
        MISS_PROCESS => "process refused",
        MISS_DEADLINE => "deadline",
        MISS_SUBMIT => "submit failed",
        MISS_EXITED => "helper exited",
        _ => "unknown",
    }
}

#[cfg(test)]
#[path = "audio_log_tests.rs"]
mod tests;
