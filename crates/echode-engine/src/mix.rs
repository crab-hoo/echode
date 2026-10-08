//! Fader after the preview cache: gain, pan, mute, solo, and peaks.
//!
//! The cache stores the signal before this stage, so moving a fader does not
//! invalidate it. This function does not allocate. The number of tracks is the
//! length of the slices the caller passes, not a fixed cap.

use crate::transport::PlaybackError;

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

/// Sums mono track planes into a stereo master.
///
/// Pan is equal-power, so a centered track keeps the same power it had in mono.
/// If any track is soloed, only soloed tracks are heard. Mute silences a track
/// even when it is soloed. `track_peaks` receives each track's louder channel
/// after the fader. The returned value is the master's louder channel.
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
        let (left_gain, right_gain) = if audible {
            pan_gains(fader.pan, fader.gain)
        } else {
            (0.0, 0.0)
        };
        let mut peak: f32 = 0.0;
        for (sample_index, sample) in plane.iter().enumerate() {
            let left_sample = sample * left_gain;
            let right_sample = sample * right_gain;
            left[sample_index] += left_sample;
            right[sample_index] += right_sample;
            peak = peak.max(left_sample.abs()).max(right_sample.abs());
        }
        track_peaks[index] = peak;
    }
    Ok(master_peak(left, right))
}

fn pan_gains(pan: f32, gain: f32) -> (f32, f32) {
    if pan <= -1.0 {
        return (gain, 0.0);
    }
    if pan >= 1.0 {
        return (0.0, gain);
    }
    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    (gain * angle.cos(), gain * angle.sin())
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
