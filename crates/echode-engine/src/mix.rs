//! Fader after the preview cache: gain, pan, mute, solo, and peaks.
//!
//! The cache stores the signal before this stage, so moving a fader does not
//! invalidate it. This function does not allocate. Tracks are added in the
//! order the caller passes them, and channels in layout order, left to right.
//! The number of tracks and channels is the length of the slices the caller
//! passes, not a fixed cap.

use crate::PlaybackError;
use dasp_interpolate::{Interpolator, linear::Linear};

/// A supported track bus. A mono source is promoted at equal power when it
/// enters a stereo bus; stereo channels are never averaged together.
#[derive(Clone, Copy)]
pub(crate) struct Signal<'a> {
    pub left: &'a [f32],
    pub right: Option<&'a [f32]>,
}

impl Signal<'_> {
    pub(crate) fn sample(self, channel: usize, frame: usize, stereo: bool) -> f32 {
        if let Some(right) = self.right {
            let plane = if channel == 0 { self.left } else { right };
            plane.get(frame).copied().unwrap_or(0.0)
        } else {
            self.left.get(frame).copied().unwrap_or(0.0)
                * if stereo {
                    std::f32::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                }
        }
    }
}

/// Sample-counted gain ramps survive plan replacement. The four gains keep
/// mono equal-power pan distinct from stereo balance even when a helper changes
/// the track's output layout. Interpolation uses the existing dasp dependency.
#[derive(Clone, Copy)]
pub(crate) struct FaderState {
    current: [f32; 4],
    start: [f32; 4],
    target: [f32; 4],
    elapsed: u32,
    frames: u32,
}

impl FaderState {
    pub(crate) fn new(fader: TrackFader, rate: u32) -> Self {
        let target = fader_gains(fader);
        Self {
            current: target,
            start: target,
            target,
            elapsed: 0,
            frames: fader_ramp_frames(rate),
        }
    }

    pub(crate) fn carry(&mut self, previous: Self) {
        let target = self.target;
        *self = previous;
        if self.target != target {
            self.start = self.current;
            self.target = target;
            self.elapsed = 0;
        }
    }

    pub(crate) fn settle(&mut self) {
        self.current = self.target;
        self.start = self.target;
        self.elapsed = self.frames;
    }

    pub(crate) fn retarget(&mut self, fader: TrackFader) {
        let target = fader_gains(fader);
        if self.target != target {
            self.start = self.current;
            self.target = target;
            self.elapsed = 0;
        }
    }

    pub(crate) fn add(&mut self, signal: Signal<'_>, left: &mut [f64], right: &mut [f64]) -> f32 {
        let mut peak = 0.0f32;
        let stereo = signal.right.is_some();
        let sources = signal.left.iter().zip(signal.right.unwrap_or(signal.left));
        for ((left, right), (source_left, source_right)) in left.iter_mut().zip(right).zip(sources)
        {
            if self.current != self.target {
                self.elapsed = self.elapsed.saturating_add(1).min(self.frames);
                self.current = if self.elapsed == self.frames {
                    self.target
                } else {
                    Linear::new(self.start, self.target)
                        .interpolate(f64::from(self.elapsed) / f64::from(self.frames))
                };
            }
            let [mono_left, mono_right, stereo_left, stereo_right] = self.current;
            let (l, r) = if stereo {
                (
                    scaled(*source_left, stereo_left),
                    scaled(*source_right, stereo_right),
                )
            } else {
                (
                    scaled(*source_left, mono_left),
                    scaled(*source_left, mono_right),
                )
            };
            *left += f64::from(l);
            *right += f64::from(r);
            peak = peak.max(l.abs()).max(r.abs());
        }
        peak
    }
}

/// A 0 or 1 gain that moves over the fader's 5 ms, for a track's own audio.
///
/// A parent that only passes a soloed child keeps its fader open and closes
/// this, so its own placed audio and instrument fall silent while the child
/// still goes through its effects and fader (ADR 0244, ADR 0338). It uses the
/// same dasp interpolation and frame count as [`FaderState`].
#[derive(Clone, Copy)]
pub(crate) struct GateRamp {
    current: [f32; 1],
    start: [f32; 1],
    target: [f32; 1],
    elapsed: u32,
    frames: u32,
}

impl GateRamp {
    pub(crate) fn new(open: bool, rate: u32) -> Self {
        let target = [if open { 1.0 } else { 0.0 }];
        // The fader's frame count, so both ramps of a gated parent end together.
        let frames = fader_ramp_frames(rate);
        Self {
            current: target,
            start: target,
            target,
            elapsed: 0,
            frames,
        }
    }

    /// Continues `previous` toward this gate's target, like [`FaderState::carry`].
    pub(crate) fn carry(&mut self, previous: Self) {
        let target = self.target;
        *self = previous;
        if self.target != target {
            self.start = self.current;
            self.target = target;
            self.elapsed = 0;
        }
    }

    /// Writes one gain per frame and advances the ramp. Returns false, and
    /// writes nothing, when every gain of the block is 1.
    pub(crate) fn fill(&mut self, gains: &mut [f32]) -> bool {
        if self.current == [1.0] && self.target == [1.0] {
            return false;
        }
        for gain in gains {
            if self.current != self.target {
                self.elapsed = self.elapsed.saturating_add(1).min(self.frames);
                self.current = if self.elapsed == self.frames {
                    self.target
                } else {
                    Linear::new(self.start, self.target)
                        .interpolate(f64::from(self.elapsed) / f64::from(self.frames))
                };
            }
            *gain = self.current[0];
        }
        true
    }
}

fn fader_ramp_frames(rate: u32) -> u32 {
    // Fader response is independent of the audio-clip loop seam policy.
    (u64::from(rate) * 5 / 1_000).max(1) as u32
}

fn scaled(sample: f32, gain: f32) -> f32 {
    if gain == 0.0 || !sample.is_finite() {
        return 0.0;
    }
    let scaled = sample * gain;
    if scaled.is_finite() { scaled } else { 0.0 }
}

fn fader_gains(fader: TrackFader) -> [f32; 4] {
    let gain = if fader.mute { 0.0 } else { fader.gain };
    let (ml, mr) = pan_gains(fader.pan, gain);
    let (sl, sr) = balance(fader.pan, gain);
    [ml, mr, sl, sr]
}

/// One track's fader. Gain is linear amplitude, and 1 is unity.
/// Pan is -1 (left) through 0 (center) to 1 (right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackFader {
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl TrackFader {
    pub const fn unity() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

/// A speaker in a bus layout. The slice order is the channel order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaker {
    Mono,
    Left,
    Right,
}

/// One channel, in the only order a mono bus uses.
pub const MONO: &[Speaker] = &[Speaker::Mono];

/// Left then right.
pub const STEREO: &[Speaker] = &[Speaker::Left, Speaker::Right];

/// One track's output. `channels` has one plane per speaker in `layout`.
#[derive(Clone, Copy)]
pub struct InputBus<'a> {
    pub layout: &'a [Speaker],
    pub channels: &'a [&'a [f32]],
}

/// Sums track buses into a stereo master.
///
/// A mono bus uses equal-power pan. A stereo bus uses balance, so center
/// leaves both channels at the fader gain. Any other layout is rejected.
/// If any track is soloed, only soloed tracks are heard. Mute silences a track
/// even when it is soloed. `track_peaks` receives each track's loudest channel
/// after the fader. The returned value is the master's loudest channel.
pub fn mix_buses(
    tracks: &[InputBus<'_>],
    faders: &[TrackFader],
    master: &mut [&mut [f32]],
    track_peaks: &mut [f32],
) -> Result<f32, PlaybackError> {
    if tracks.len() != faders.len() || tracks.len() != track_peaks.len() {
        return Err(PlaybackError::BufferTooSmall);
    }
    let [left, right] = master else {
        return Err(PlaybackError::BufferTooSmall);
    };
    let frames = left.len();
    if right.len() != frames {
        return Err(PlaybackError::BufferTooSmall);
    }
    for fader in faders {
        if !(-1.0..=1.0).contains(&fader.pan) || !fader.gain.is_finite() {
            return Err(PlaybackError::InvalidFader);
        }
    }
    for track in tracks {
        if !supported(track, frames) {
            return Err(PlaybackError::BufferTooSmall);
        }
    }

    left.fill(0.0);
    right.fill(0.0);
    let any_solo = faders.iter().any(|fader| fader.solo);
    for ((track, fader), peak) in tracks.iter().zip(faders).zip(track_peaks) {
        let audible = !fader.mute && (!any_solo || fader.solo);
        *peak = if audible {
            add_track(track, fader, left, right)
        } else {
            0.0
        };
    }
    Ok(master_peak(left, right))
}

/// Sums mono track planes into a stereo master.
///
/// Each plane is a mono bus. Pan is equal-power.
pub fn mix_tracks(
    planes: &[&[f32]],
    faders: &[TrackFader],
    left: &mut [f32],
    right: &mut [f32],
    track_peaks: &mut [f32],
) -> Result<f32, PlaybackError> {
    if planes.len() != faders.len() || planes.len() != track_peaks.len() {
        return Err(PlaybackError::BufferTooSmall);
    }
    if left.len() != right.len() {
        return Err(PlaybackError::BufferTooSmall);
    }
    for fader in faders {
        if !(-1.0..=1.0).contains(&fader.pan) || !fader.gain.is_finite() {
            return Err(PlaybackError::InvalidFader);
        }
    }
    for plane in planes {
        if plane.len() != left.len() {
            return Err(PlaybackError::BufferTooSmall);
        }
    }
    left.fill(0.0);
    right.fill(0.0);
    let any_solo = faders.iter().any(|fader| fader.solo);
    for ((plane, fader), peak) in planes.iter().zip(faders).zip(track_peaks) {
        let audible = !fader.mute && (!any_solo || fader.solo);
        let bus = InputBus {
            layout: MONO,
            channels: std::slice::from_ref(plane),
        };
        *peak = if audible {
            add_track(&bus, fader, left, right)
        } else {
            0.0
        };
    }
    Ok(master_peak(left, right))
}

fn supported(track: &InputBus<'_>, frames: usize) -> bool {
    let mono = track.layout == MONO && track.channels.len() == 1;
    let stereo = track.layout == STEREO && track.channels.len() == 2;
    (mono || stereo) && track.channels.iter().all(|plane| plane.len() == frames)
}

/// `track` is a shape [`supported`] accepts.
fn add_track(track: &InputBus<'_>, fader: &TrackFader, left: &mut [f32], right: &mut [f32]) -> f32 {
    match track.channels {
        [plane] if track.layout == MONO => {
            let (left_gain, right_gain) = pan_gains(fader.pan, fader.gain);
            add_mono(plane, left_gain, right_gain, left, right)
        }
        [source_left, source_right] => {
            let (left_gain, right_gain) = balance(fader.pan, fader.gain);
            add_stereo(
                source_left,
                source_right,
                left_gain,
                right_gain,
                left,
                right,
            )
        }
        _ => 0.0,
    }
}

fn add_mono(
    plane: &[f32],
    left_gain: f32,
    right_gain: f32,
    left: &mut [f32],
    right: &mut [f32],
) -> f32 {
    let mut peak = 0.0f32;
    for ((sample, left), right) in plane.iter().zip(left).zip(right) {
        let left_sample = sample * left_gain;
        let right_sample = sample * right_gain;
        *left += left_sample;
        *right += right_sample;
        peak = peak.max(left_sample.abs()).max(right_sample.abs());
    }
    peak
}

fn add_stereo(
    source_left: &[f32],
    source_right: &[f32],
    left_gain: f32,
    right_gain: f32,
    left: &mut [f32],
    right: &mut [f32],
) -> f32 {
    let mut peak = 0.0f32;
    let sources = source_left.iter().zip(source_right);
    for ((source_left, source_right), (left, right)) in sources.zip(left.iter_mut().zip(right)) {
        let left_sample = source_left * left_gain;
        let right_sample = source_right * right_gain;
        *left += left_sample;
        *right += right_sample;
        peak = peak.max(left_sample.abs()).max(right_sample.abs());
    }
    peak
}

fn balance(pan: f32, gain: f32) -> (f32, f32) {
    if pan <= -1.0 {
        (gain, 0.0)
    } else if pan >= 1.0 {
        (0.0, gain)
    } else if pan < 0.0 {
        (gain, gain * (pan + 1.0))
    } else {
        (gain * (1.0 - pan), gain)
    }
}

fn pan_gains(pan: f32, gain: f32) -> (f32, f32) {
    if pan <= -1.0 {
        return (gain, 0.0);
    }
    if pan >= 1.0 {
        return (0.0, gain);
    }
    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    (gain * libm::cosf(angle), gain * libm::sinf(angle))
}

fn master_peak(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .chain(right)
        .fold(0.0, |peak, sample| peak.max(sample.abs()))
}

#[cfg(test)]
#[path = "mix_tests.rs"]
mod tests;
