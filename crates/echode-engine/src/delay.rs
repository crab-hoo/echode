//! Delay lines that keep paths with different plugin latency in time (ADR 0339).
//!
//! Control sizes every line for a plan. The callback writes and reads within
//! that capacity, clears in O(1) and takes over the previous plan's history
//! in O(1), so it never allocates, frees or copies a history. A zero delay has
//! no buffer and leaves the signal as it is.

use crate::PlaybackError;

/// A stereo delay of a fixed number of frames.
#[derive(Debug, Default)]
pub(crate) struct Delay {
    ring: Ring,
    /// The previous plan's ring, when its capacity differed from this one's.
    /// Frames written before the carry are read from it until `ring` holds
    /// them all. It returns to control with this line's plan.
    older: Ring,
    delay: usize,
}

#[derive(Debug, Default)]
struct Ring {
    left: Vec<f32>,
    right: Vec<f32>,
    /// Index of the next frame to write, below the capacity.
    write: usize,
    /// Frames written since this ring was cleared or first carried, up to its
    /// capacity. A read older than this is not in this ring.
    filled: usize,
}

impl Ring {
    fn zeroed(capacity: usize, delay: u64) -> Result<Self, PlaybackError> {
        Ok(Self {
            left: zeroed(capacity, delay)?,
            right: zeroed(capacity, delay)?,
            write: 0,
            filled: 0,
        })
    }

    fn capacity(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    /// The frame written `age` frames ago, 1 being the newest.
    fn read(&self, age: usize) -> Option<(f32, f32)> {
        let capacity = self.capacity();
        if age == 0 || age > self.filled || age > capacity {
            return None;
        }
        // Every ring's capacity is a power of two, from `Delay::new`.
        let index = self.write.wrapping_sub(age) & (capacity - 1);
        self.left
            .get(index)
            .copied()
            .zip(self.right.get(index).copied())
    }
}

impl Delay {
    /// Holds `delay` frames plus one block of up to `block` frames in a
    /// power-of-two ring. A whole block is then written before it is read, and
    /// a later plan whose delay changes within the same capacity swaps this
    /// ring in.
    pub(crate) fn new(delay: u64, block: usize) -> Result<Self, PlaybackError> {
        if delay == 0 {
            return Ok(Self::default());
        }
        let frames = usize::try_from(delay).map_err(|_| PlaybackError::Latency(delay))?;
        let capacity = frames
            .checked_add(block.max(1))
            .and_then(usize::checked_next_power_of_two)
            .ok_or(PlaybackError::Latency(delay))?;
        Ok(Self {
            ring: Ring::zeroed(capacity, delay)?,
            older: Ring::default(),
            delay: frames,
        })
    }

    pub(crate) fn frames(&self) -> usize {
        self.delay
    }

    /// Delays `left`, and `right` when given, in place. A mono block also
    /// writes its left plane as the right history, so a later stereo block
    /// reads that signal rather than an older one.
    pub(crate) fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let capacity = self.ring.capacity();
        if self.delay == 0 || capacity <= self.delay {
            return;
        }
        // A part no longer than the room past the delay never overwrites a
        // frame it still reads. Blocks up to the planned size are one part.
        let room = capacity - self.delay;
        let mut start = 0;
        while start < left.len() {
            let end = left.len().min(start + room);
            let Some(part) = left.get_mut(start..end) else {
                return;
            };
            match right.as_deref_mut() {
                Some(right) => {
                    let Some(right) = right.get_mut(start..end) else {
                        return;
                    };
                    self.part(part, Some(right));
                }
                None => self.part(part, None),
            }
            start = end;
        }
    }

    /// One part of at most `capacity - delay` frames: its input is written,
    /// then its output read as at most two slices of the ring each.
    fn part(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let frames = left.len();
        let mask = self.ring.capacity() - 1;
        let (write, filled) = (self.ring.write, self.ring.filled);
        put(&mut self.ring.left, write, left);
        put(
            &mut self.ring.right,
            write,
            right.as_deref().unwrap_or(left),
        );
        // Outputs before `fresh` were written before this ring's carry or
        // clear: they come from the older ring, or are silence.
        let fresh = self.delay.saturating_sub(filled).min(frames);
        let from = write.wrapping_add(fresh).wrapping_sub(self.delay) & mask;
        if let Some(rest) = left.get_mut(fresh..) {
            take(&self.ring.left, from, rest);
        }
        if let Some(rest) = right
            .as_deref_mut()
            .and_then(|right| right.get_mut(fresh..))
        {
            take(&self.ring.right, from, rest);
        }
        for index in 0..fresh {
            let age = self.delay - filled - index;
            let (old_left, old_right) = self.older.read(age).unwrap_or((0.0, 0.0));
            if let Some(sample) = left.get_mut(index) {
                *sample = old_left;
            }
            if let Some(sample) = right.as_deref_mut().and_then(|right| right.get_mut(index)) {
                *sample = old_right;
            }
        }
        self.ring.write = write.wrapping_add(frames) & mask;
        self.ring.filled = filled.saturating_add(frames).min(mask + 1);
    }

    /// Silences what the line holds, at the transport discontinuities that
    /// also reset the plugins. The buffers are left as they are.
    pub(crate) fn clear(&mut self) {
        self.ring.filled = 0;
        self.older.filled = 0;
    }

    /// Continues the previous plan's line for the same path, in O(1).
    ///
    /// With the same capacity the rings are swapped. Otherwise the previous
    /// ring is kept to read the frames written before the carry; history the
    /// previous line itself still read from its own older ring is dropped.
    /// The previous line returns to control holding this line's spare buffers.
    pub(crate) fn carry(&mut self, previous: &mut Self) {
        if self.ring.capacity() == 0 {
            return;
        }
        if previous.ring.capacity() == self.ring.capacity() {
            std::mem::swap(&mut self.ring, &mut previous.ring);
            std::mem::swap(&mut self.older, &mut previous.older);
        } else {
            std::mem::swap(&mut self.older, &mut previous.ring);
        }
    }
}

/// Copies `source` into the ring from `at`, wrapping once at its end.
fn put(ring: &mut [f32], at: usize, source: &[f32]) {
    let split = ring.len().saturating_sub(at).min(source.len());
    let (first, second) = source.split_at(split);
    if let Some(slot) = ring.get_mut(at..at + split) {
        slot.copy_from_slice(first);
    }
    if let Some(slot) = ring.get_mut(..second.len()) {
        slot.copy_from_slice(second);
    }
}

/// Copies the ring from `at` into `target`, wrapping once at its end.
fn take(ring: &[f32], at: usize, target: &mut [f32]) {
    let split = ring.len().saturating_sub(at).min(target.len());
    let (first, second) = target.split_at_mut(split);
    if let Some(slot) = ring.get(at..at + split) {
        first.copy_from_slice(slot);
    }
    if let Some(slot) = ring.get(..second.len()) {
        second.copy_from_slice(slot);
    }
}

/// A zeroed plane, or an error instead of aborting when memory is short.
fn zeroed(capacity: usize, delay: u64) -> Result<Vec<f32>, PlaybackError> {
    let mut plane = Vec::new();
    plane
        .try_reserve_exact(capacity)
        .map_err(|_| PlaybackError::Latency(delay))?;
    plane.resize(capacity, 0.0);
    Ok(plane)
}

#[cfg(test)]
#[path = "delay_tests.rs"]
mod tests;
