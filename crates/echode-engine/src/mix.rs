//! Fader after the preview cache: gain, pan, mute, solo, and peaks.
//!
//! The cache stores the signal before this stage, so moving a fader does not
//! invalidate it. This function does not allocate. Tracks are added in the
//! order the caller passes them, and channels in layout order, left to right.
//! The number of tracks and channels is the length of the slices the caller
//! passes, not a fixed cap.

use crate::transport::PlaybackError;
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
        for (frame, (left, right)) in left.iter_mut().zip(right).enumerate() {
            if self.current != self.target {
                self.elapsed = self.elapsed.saturating_add(1).min(self.frames);
                self.current = if self.elapsed == self.frames {
                    self.target
                } else {
                    Linear::new(self.start, self.target)
                        .interpolate(f64::from(self.elapsed) / f64::from(self.frames))
                };
            }
            let (l, r) = if let Some(source_right) = signal.right {
                (
                    scaled(signal.left[frame], self.current[2]),
                    scaled(source_right[frame], self.current[3]),
                )
            } else {
                (
                    scaled(signal.left[frame], self.current[0]),
                    scaled(signal.left[frame], self.current[1]),
                )
            };
            *left += f64::from(l);
            *right += f64::from(r);
            peak = peak.max(l.abs()).max(r.abs());
        }
        peak
    }
}

fn fader_ramp_frames(rate: u32) -> u32 {
    // Fader response is independent of the audio-clip loop seam policy.
    (u64::from(rate) * 5 / 1_000).max(1) as u32
}

fn scaled(sample: f32, gain: f32) -> f32 {
    if gain == 0.0 { 0.0 } else { sample * gain }
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
    if master.len() != STEREO.len() {
        return Err(PlaybackError::BufferTooSmall);
    }
    let frames = master[0].len();
    if master[1].len() != frames {
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

    master[0].fill(0.0);
    master[1].fill(0.0);
    let any_solo = faders.iter().any(|fader| fader.solo);
    let (left, rest) = master.split_first_mut().expect("stereo master");
    let right = &mut *rest[0];
    for (index, (track, fader)) in tracks.iter().zip(faders).enumerate() {
        let audible = !fader.mute && (!any_solo || fader.solo);
        track_peaks[index] = if audible {
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
    for (index, (plane, fader)) in planes.iter().zip(faders).enumerate() {
        let audible = !fader.mute && (!any_solo || fader.solo);
        let bus = InputBus {
            layout: MONO,
            channels: std::slice::from_ref(plane),
        };
        track_peaks[index] = if audible {
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

fn add_track(track: &InputBus<'_>, fader: &TrackFader, left: &mut [f32], right: &mut [f32]) -> f32 {
    if track.layout == MONO {
        let (left_gain, right_gain) = pan_gains(fader.pan, fader.gain);
        return add_mono(track.channels[0], left_gain, right_gain, left, right);
    }
    let (left_gain, right_gain) = balance(fader.pan, fader.gain);
    add_stereo(
        track.channels[0],
        track.channels[1],
        left_gain,
        right_gain,
        left,
        right,
    )
}

fn add_mono(
    plane: &[f32],
    left_gain: f32,
    right_gain: f32,
    left: &mut [f32],
    right: &mut [f32],
) -> f32 {
    let mut peak = 0.0f32;
    for (index, sample) in plane.iter().enumerate() {
        let left_sample = sample * left_gain;
        let right_sample = sample * right_gain;
        left[index] += left_sample;
        right[index] += right_sample;
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
    for index in 0..source_left.len() {
        let left_sample = source_left[index] * left_gain;
        let right_sample = source_right[index] * right_gain;
        left[index] += left_sample;
        right[index] += right_sample;
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
mod tests {
    use super::*;

    #[test]
    fn center_pan_keeps_power_and_hard_pan_picks_one_side() {
        let plane = [1.0, -0.5];
        let mut left = [0.0; 2];
        let mut right = [0.0; 2];
        let mut peaks = [0.0; 1];
        let master = mix_tracks(
            &[&plane],
            &[TrackFader::unity()],
            &mut left,
            &mut right,
            &mut peaks,
        )
        .unwrap();
        assert!((left[0] - right[0]).abs() < 1e-6);
        assert!((left[0] * left[0] + right[0] * right[0] - 1.0).abs() < 1e-5);
        assert!((peaks[0] - left[0].abs()).abs() < 1e-6);
        assert!((master - peaks[0]).abs() < 1e-6);

        let hard_left = TrackFader {
            pan: -1.0,
            ..TrackFader::unity()
        };
        mix_tracks(&[&plane], &[hard_left], &mut left, &mut right, &mut peaks).unwrap();
        assert_eq!(left[0], 1.0);
        assert_eq!(right[0], 0.0);
        assert_eq!(left[1], -0.5);
    }

    #[test]
    fn a_centered_stereo_track_keeps_its_sides_and_a_hard_left_drops_the_right() {
        let source_left = [0.5, -0.25];
        let source_right = [0.125, 0.75];
        let channels: [&[f32]; 2] = [&source_left, &source_right];
        let track = InputBus {
            layout: STEREO,
            channels: &channels,
        };
        let mut left = [0.0; 2];
        let mut right = [0.0; 2];
        let mut peaks = [0.0; 1];
        {
            let mut master: [&mut [f32]; 2] = [&mut left, &mut right];
            mix_buses(&[track], &[TrackFader::unity()], &mut master, &mut peaks).unwrap();
        }
        assert_eq!(left, source_left);
        assert_eq!(right, source_right);

        let hard_left = TrackFader {
            pan: -1.0,
            gain: 0.5,
            ..TrackFader::unity()
        };
        {
            let mut master: [&mut [f32]; 2] = [&mut left, &mut right];
            mix_buses(&[track], &[hard_left], &mut master, &mut peaks).unwrap();
        }
        assert_eq!(left, [0.25, -0.125]);
        assert_eq!(right, [0.0, 0.0]);
        assert!((peaks[0] - 0.25).abs() < 1e-6);

        let odd_layout = [Speaker::Left, Speaker::Right, Speaker::Mono];
        let odd_channels: [&[f32]; 3] = [&source_left, &source_right, &source_left];
        let odd = InputBus {
            layout: &odd_layout,
            channels: &odd_channels,
        };
        let mut master: [&mut [f32]; 2] = [&mut left, &mut right];
        assert_eq!(
            mix_buses(&[odd], &[TrackFader::unity()], &mut master, &mut peaks).unwrap_err(),
            PlaybackError::BufferTooSmall
        );
    }

    #[test]
    fn mute_and_solo_choose_who_reaches_the_master() {
        let a = [1.0];
        let b = [1.0];
        let mut left = [0.0; 1];
        let mut right = [0.0; 1];
        let mut peaks = [0.0; 2];
        let muted = TrackFader {
            mute: true,
            pan: -1.0,
            ..TrackFader::unity()
        };
        let open = TrackFader {
            pan: 1.0,
            ..TrackFader::unity()
        };
        mix_tracks(&[&a, &b], &[muted, open], &mut left, &mut right, &mut peaks).unwrap();
        assert_eq!(left[0], 0.0);
        assert_eq!(right[0], 1.0);
        assert_eq!(peaks[0], 0.0);

        let solo_muted = TrackFader {
            mute: true,
            solo: true,
            pan: -1.0,
            ..TrackFader::unity()
        };
        let other = TrackFader {
            pan: 1.0,
            ..TrackFader::unity()
        };
        let master = mix_tracks(
            &[&a, &b],
            &[solo_muted, other],
            &mut left,
            &mut right,
            &mut peaks,
        )
        .unwrap();
        assert_eq!(master, 0.0);

        let solo = TrackFader {
            solo: true,
            pan: -1.0,
            ..TrackFader::unity()
        };
        mix_tracks(&[&a, &b], &[solo, other], &mut left, &mut right, &mut peaks).unwrap();
        assert_eq!(left[0], 1.0);
        assert_eq!(right[0], 0.0);
        assert_eq!(peaks[1], 0.0);
    }

    #[test]
    fn two_tracks_sum_and_a_bad_fader_is_rejected() {
        let a = [0.25];
        let b = [0.25];
        let mut left = [0.0; 1];
        let mut right = [0.0; 1];
        let mut peaks = [0.0; 2];
        let half = TrackFader {
            gain: 0.5,
            pan: -1.0,
            ..TrackFader::unity()
        };
        mix_tracks(&[&a, &b], &[half, half], &mut left, &mut right, &mut peaks).unwrap();
        assert!((left[0] - 0.25).abs() < 1e-6);
        assert_eq!(right[0], 0.0);

        let wild = TrackFader {
            pan: 1.5,
            ..TrackFader::unity()
        };
        assert_eq!(
            mix_tracks(&[&a], &[wild], &mut left, &mut right, &mut peaks[..1]).unwrap_err(),
            PlaybackError::InvalidFader
        );
        assert_eq!(
            mix_tracks(&[&a, &b], &[half], &mut left, &mut right, &mut peaks).unwrap_err(),
            PlaybackError::BufferTooSmall
        );
    }
}
