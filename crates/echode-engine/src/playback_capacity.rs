//! Control-thread event bounds from local density, not score length.
use super::PlaybackError;
use echode_realtime::TrackPlan;

pub(super) struct TrackEventDemand {
    pub(super) key: u32,
    pub(super) events: usize,
    pub(super) chase: usize,
    pub(super) previous_chase: usize,
}

pub(super) fn track_event_demand(
    track: &TrackPlan,
    bounds: Option<(i64, i64)>,
    frames: usize,
) -> Result<TrackEventDemand, PlaybackError> {
    if frames == 0 {
        return Err(PlaybackError::BufferTooSmall);
    }
    // Both orders already belong to the plan; merge once rather than sorting
    // or expanding one copy of the score per possible loop iteration.
    let mut ons = track.notes.iter().map(|note| note.on).peekable();
    let mut offs = track
        .by_off
        .iter()
        .filter_map(|index| track.notes.get(*index as usize).map(|note| note.off))
        .peekable();
    let edges: Vec<_> = std::iter::from_fn(|| match (ons.peek(), offs.peek()) {
        (Some(on), Some(off)) if off <= on => offs.next(),
        (Some(_), _) => ons.next(),
        (None, _) => offs.next(),
    })
    .collect();
    let mut first = 0;
    let mut block = 0;
    for (last, &edge) in edges.iter().enumerate() {
        while edges
            .get(first)
            .is_some_and(|first| i128::from(edge) - i128::from(*first) >= frames as i128)
        {
            first += 1;
        }
        block = block.max(last + 1 - first);
    }
    // Offs precede ons at the same sample. Chasing an exact end may hold that
    // voice for its off, but does not also chase notes starting at that sample.
    let mut ended = 0;
    let mut overlap = 0;
    for (index, note) in track.notes.iter().enumerate() {
        while track
            .by_off
            .get(ended)
            .and_then(|off| track.notes.get(*off as usize))
            .is_some_and(|off| off.off <= note.on)
        {
            ended += 1;
        }
        overlap = overlap.max(index + 1 - ended);
    }
    if let Some((start, end)) = bounds {
        if end <= start {
            return Err(PlaybackError::InvalidLoop);
        }
        let length = i128::from(end) - i128::from(start);
        // A wrap releases every voice still held at the excluded loop end,
        // including voices chased on the first pass from before the loop.
        let releases = track
            .notes
            .iter()
            .filter(|note| note.on < end && note.off >= end)
            .count();
        let circular: Vec<_> = std::iter::repeat_n(0, releases)
            .chain(
                edges
                    .iter()
                    .copied()
                    .filter(|edge| *edge >= start && *edge < end)
                    .map(|edge| i128::from(edge) - i128::from(start)),
            )
            .collect();
        let cycles = usize::try_from(frames as i128 / length).map_err(|_| PlaybackError::Time)?;
        let remainder = frames as i128 % length;
        let mut partial = 0;
        let mut first = 0;
        let count = circular.len();
        let circular_at = |index: usize| {
            circular
                .get(index % count)
                .map(|at| *at + if index >= count { length } else { 0 })
                .ok_or(PlaybackError::Time)
        };
        if remainder != 0 {
            for last in 0..count.checked_mul(2).ok_or(PlaybackError::Time)? {
                let at = circular_at(last)?;
                while last - first >= count || at - circular_at(first)? >= remainder {
                    first += 1;
                }
                partial = partial.max(last + 1 - first);
            }
        }
        let looped = count
            .checked_mul(cycles)
            .and_then(|n| n.checked_add(partial))
            .ok_or(PlaybackError::Time)?;
        // A block may enter from before the loop, so reserve its linear prefix
        // as well as a full block of the circular path. No repeated score copy.
        block = block.checked_add(looped).ok_or(PlaybackError::Time)?;
    }
    Ok(TrackEventDemand {
        key: track.track_key,
        // A locate can release the previous live set before this block's edges.
        events: block.checked_add(overlap).ok_or(PlaybackError::Time)?,
        chase: overlap,
        previous_chase: 0,
    })
}

#[cfg(test)]
#[path = "playback_capacity_tests.rs"]
mod tests;
