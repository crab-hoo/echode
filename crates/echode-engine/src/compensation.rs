//! Plugin delay compensation, planned on control (ADR 0339).
//!
//! Every path to master is delayed to the longest one. A track's placed audio
//! and first instrument meet its children's sum before its next device, so
//! they are aligned there, and every track is aligned again where it meets
//! its siblings: at its parent's input, or at master. Master then lags the
//! timeline by one latency, which an export removes.
//!
//! A bypassed device keeps its latency. Its dry audio is delayed at the
//! track's output instead, so bypassing a device moves no other track.

use crate::PlaybackError;

/// Reported latency of one track's devices, in device frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TrackLatency {
    /// The first device, when it is an instrument, bypassed or not.
    pub(crate) instrument: u64,
    /// Every later device, bypassed or not.
    pub(crate) chain: u64,
    /// The part of `chain` whose devices are bypassed and pass audio at once.
    pub(crate) bypassed: u64,
    /// Declared tails of the devices that process, summed. `u32::MAX` is
    /// infinite and stays so.
    pub(crate) tail: u32,
}

/// Delays for one track, in device frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TrackDelays {
    /// The track's own placed audio, to meet its instrument and children.
    pub(crate) source: u64,
    /// The first instrument's output, to meet the placed audio and children.
    pub(crate) instrument: u64,
    /// The track's output before its fader, to meet its siblings.
    pub(crate) output: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Compensation {
    /// In plan order.
    pub(crate) tracks: Vec<TrackDelays>,
    /// How far master lags the timeline.
    pub(crate) latency: u64,
    /// The longest declared tail along any path to master.
    pub(crate) tail: u32,
}

/// `parents` and `latencies` are in plan order, where every parent precedes
/// its children. Two passes over the tracks, without recursion, so the depth
/// of the folders is not limited.
pub(crate) fn compensate(
    parents: &[Option<usize>],
    latencies: &[TrackLatency],
) -> Result<Compensation, PlaybackError> {
    let count = parents.len();
    if latencies.len() != count {
        return Err(PlaybackError::BufferTooSmall);
    }
    // The latest child of each track at its input, then the meeting point of
    // its own audio and its children, and its arrival after its devices.
    let mut children = vec![0u64; count];
    let mut meets = vec![0u64; count];
    let mut arrivals = vec![0u64; count];
    let mut latency = 0u64;
    for position in (0..count).rev() {
        let (Some(track), Some(input)) = (latencies.get(position), children.get(position)) else {
            return Err(PlaybackError::BufferTooSmall);
        };
        let meet = track.instrument.max(*input);
        let arrival = meet
            .checked_add(track.chain)
            .ok_or(PlaybackError::Latency(u64::MAX))?;
        if let (Some(slot), Some(at)) = (meets.get_mut(position), arrivals.get_mut(position)) {
            *slot = meet;
            *at = arrival;
        }
        match parents.get(position).copied().flatten() {
            Some(parent) if parent < position => {
                let latest = children
                    .get_mut(parent)
                    .ok_or(PlaybackError::BufferTooSmall)?;
                *latest = (*latest).max(arrival);
            }
            Some(_) => return Err(PlaybackError::BufferTooSmall),
            None => latency = latency.max(arrival),
        }
    }
    let mut tracks = Vec::with_capacity(count);
    let mut tails = vec![0u32; count];
    let mut tail = 0u32;
    for position in 0..count {
        let (Some(track), Some(meet), Some(arrival)) = (
            latencies.get(position),
            meets.get(position),
            arrivals.get(position),
        ) else {
            return Err(PlaybackError::BufferTooSmall);
        };
        let parent = parents.get(position).copied().flatten();
        let target = match parent {
            Some(parent) => *meets.get(parent).ok_or(PlaybackError::BufferTooSmall)?,
            None => latency,
        };
        let output = target
            .checked_sub(*arrival)
            .and_then(|early| early.checked_add(track.bypassed))
            .ok_or(PlaybackError::Latency(u64::MAX))?;
        tracks.push(TrackDelays {
            source: *meet,
            instrument: meet.saturating_sub(track.instrument),
            output,
        });
        let above = parent.and_then(|parent| tails.get(parent).copied());
        let path = track.tail.saturating_add(above.unwrap_or(0));
        if let Some(slot) = tails.get_mut(position) {
            *slot = path;
        }
        tail = tail.max(path);
    }
    Ok(Compensation {
        tracks,
        latency,
        tail,
    })
}

#[cfg(test)]
#[path = "compensation_tests.rs"]
mod tests;
