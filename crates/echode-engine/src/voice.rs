//! Track playback that can read a preview cache instead of live devices.
//!
//! The cache itself is rendered later. This module only swaps the source and
//! shapes loop and clip edges. It does not allocate. One call renders one
//! plane; the mixer repeats it per channel, so the channel count is not fixed.
//! Audio here is already at the device rate. Sample-rate conversion is separate.

use crate::transport::{PlaybackError, PlayedSpan};

/// Five milliseconds. Long enough to hide a waveform step, short enough to
/// leave the note intact.
const SEAM_FADE_MS: u64 = 5;

/// How many frames a 5 ms seam occupies at `sample_rate`.
pub fn seam_fade_frames(sample_rate: u32) -> u32 {
    u32::try_from(u64::from(sample_rate) * SEAM_FADE_MS / 1_000)
        .unwrap_or(u32::MAX)
        .max(1)
}

/// Where a live device chain can be replaced by cached audio.
pub enum TrackSource<'a> {
    /// Copy the block the devices just rendered.
    Live,
    /// Device output from before the fader, addressed by project sample.
    Cache { samples: &'a [f32], origin: i64 },
}

/// Samples already at the device rate, placed on the project timeline.
pub struct AudioClip<'a> {
    pub start_sample: i64,
    pub samples: &'a [f32],
}

/// The loop head a disk reader must have resident before the playhead wraps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopHead {
    pub start_sample: i64,
    pub frames: u32,
}

/// Renders one plane of a track into `output`.
///
/// `live` is ignored unless the source is [`TrackSource::Live`]. Cache samples
/// outside the stored region read as silence, which is the not-yet-rendered part.
pub fn render_track(
    source: TrackSource<'_>,
    live: &[f32],
    spans: &[PlayedSpan],
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    match source {
        TrackSource::Live => copy_block(live, spans, output),
        TrackSource::Cache { samples, origin } => read_cache(samples, origin, spans, output),
    }
}

/// Samples already at the device rate, with the placement's own edge fades.
///
/// A fade of zero leaves that edge at full level. Fades of one frame do too:
/// a ramp needs two frames.
pub struct FadedClip<'a> {
    pub start_sample: i64,
    pub samples: &'a [f32],
    pub fade_in: u32,
    pub fade_out: u32,
}

/// Adds clips into `output` using each clip's fade lengths.
///
/// The buffer is not cleared. Clips must be ordered by start sample, which is
/// the same order as their ends when placements do not overlap.
pub fn render_faded_clips(
    clips: &[FadedClip<'_>],
    spans: &[PlayedSpan],
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    for span in spans {
        let end = span
            .start_sample
            .checked_add(i64::from(span.frames))
            .ok_or(PlaybackError::Time)?;
        for clip in faded_overlapping(clips, span.start_sample, end) {
            let clip_end = clip
                .start_sample
                .saturating_add(i64::try_from(clip.samples.len()).unwrap_or(i64::MAX));
            let from = span.start_sample.max(clip.start_sample);
            let to = end.min(clip_end);
            let mut project = from;
            while project < to {
                let local =
                    u32::try_from(project - span.start_sample).map_err(|_| PlaybackError::Time)?;
                let index = block_index(span, local)?;
                if index >= output.len() {
                    return Err(PlaybackError::BufferTooSmall);
                }
                output[index] += faded_sample(clip, project);
                project += 1;
            }
        }
    }
    Ok(())
}

fn faded_overlapping<'a>(
    clips: &'a [FadedClip<'a>],
    start: i64,
    end: i64,
) -> impl Iterator<Item = &'a FadedClip<'a>> {
    let first = clips.partition_point(|clip| faded_end(clip) <= start);
    clips[first..]
        .iter()
        .take_while(move |clip| clip.start_sample < end)
}

fn faded_end(clip: &FadedClip<'_>) -> i64 {
    clip.start_sample
        .saturating_add(i64::try_from(clip.samples.len()).unwrap_or(i64::MAX))
}

fn faded_sample(clip: &FadedClip<'_>, project: i64) -> f32 {
    let Some(pos) = project.checked_sub(clip.start_sample) else {
        return 0.0;
    };
    if pos < 0 {
        return 0.0;
    }
    let Some(index) = usize::try_from(pos).ok() else {
        return 0.0;
    };
    let Some(sample) = clip.samples.get(index).copied() else {
        return 0.0;
    };
    sample
        * pair_gain(
            pos,
            clip.samples.len() as i64,
            i64::from(clip.fade_in),
            i64::from(clip.fade_out),
        )
}

fn pair_gain(pos: i64, length: i64, fade_in: i64, fade_out: i64) -> f32 {
    if length <= 0 {
        return 0.0;
    }
    let mut gain: f32 = 1.0;
    if fade_in > 1 {
        let fade = fade_in.min(length);
        let denom = (fade - 1) as f32;
        if pos < fade {
            gain = gain.min(pos as f32 / denom);
        }
    }
    if fade_out > 1 {
        let fade = fade_out.min(length);
        let denom = (fade - 1) as f32;
        let tail = length - 1 - pos;
        if tail < fade {
            gain = gain.min(tail as f32 / denom);
        }
    }
    gain
}

/// Adds audio clips into `output`, fading each edge over 5 ms.
///
/// The buffer is not cleared. Overlapping clips sum, so two clips that meet
/// crossfade through the seam.
pub fn render_audio_clips(
    clips: &[AudioClip<'_>],
    spans: &[PlayedSpan],
    sample_rate: u32,
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    let fade = i64::from(seam_fade_frames(sample_rate));
    for span in spans {
        let end = span
            .start_sample
            .checked_add(i64::from(span.frames))
            .ok_or(PlaybackError::Time)?;
        for clip in overlapping(clips, span.start_sample, end) {
            let clip_end = clip
                .start_sample
                .saturating_add(i64::try_from(clip.samples.len()).unwrap_or(i64::MAX));
            let from = span.start_sample.max(clip.start_sample);
            let to = end.min(clip_end);
            let mut project = from;
            while project < to {
                let local =
                    u32::try_from(project - span.start_sample).map_err(|_| PlaybackError::Time)?;
                let index = block_index(span, local)?;
                if index >= output.len() {
                    return Err(PlaybackError::BufferTooSmall);
                }
                output[index] += clip_sample(clip, project, fade);
                project += 1;
            }
        }
    }
    Ok(())
}

fn overlapping<'a>(
    clips: &'a [AudioClip<'a>],
    start: i64,
    end: i64,
) -> impl Iterator<Item = &'a AudioClip<'a>> {
    let first = clips.partition_point(|clip| clip_end(clip) <= start);
    clips[first..]
        .iter()
        .take_while(move |clip| clip.start_sample < end)
}

fn clip_end(clip: &AudioClip<'_>) -> i64 {
    clip.start_sample
        .saturating_add(i64::try_from(clip.samples.len()).unwrap_or(i64::MAX))
}

/// Fades the discontinuity where the playhead jumps back to the loop head.
///
/// Live devices are not faded here: their tail is meant to keep ringing into
/// the next pass. Call this on audio-clip output only.
pub fn fade_loop_seam(
    spans: &[PlayedSpan],
    fade_frames: u32,
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    if fade_frames == 0 {
        return Ok(());
    }
    let denom = (fade_frames.saturating_sub(1).max(1)) as f32;
    let window = i64::from(fade_frames);
    for span in spans {
        for local in 0..span.frames {
            let index = block_index(span, local)?;
            if index >= output.len() {
                return Err(PlaybackError::BufferTooSmall);
            }
            let project = span
                .start_sample
                .checked_add(i64::from(local))
                .ok_or(PlaybackError::Time)?;
            let mut gain = 1.0f32;
            if let Some(loop_end) = span.loop_end
                && let Some(before_end) = loop_end
                    .checked_sub(1)
                    .and_then(|end| end.checked_sub(project))
                && (0..window).contains(&before_end)
            {
                gain *= before_end as f32 / denom;
            }
            if let Some(origin) = span.wrap_from
                && let Some(age) = project.checked_sub(origin)
                && (0..window).contains(&age)
            {
                gain *= age as f32 / denom;
            }
            output[index] *= gain;
        }
    }
    Ok(())
}

/// The loop head to read ahead when the playhead is within `horizon_frames` of the end.
///
/// This does not touch the disk. `None` means the head is not needed yet.
pub fn loop_head_prefetch(
    position: i64,
    loop_start: i64,
    loop_end: i64,
    horizon_frames: u32,
    head_frames: u32,
) -> Option<LoopHead> {
    if loop_end <= loop_start || head_frames == 0 || position >= loop_end {
        return None;
    }
    let horizon_end = position.saturating_add(i64::from(horizon_frames));
    if horizon_end >= loop_end {
        Some(LoopHead {
            start_sample: loop_start,
            frames: head_frames,
        })
    } else {
        None
    }
}

fn copy_block(live: &[f32], spans: &[PlayedSpan], output: &mut [f32]) -> Result<(), PlaybackError> {
    for span in spans {
        for local in 0..span.frames {
            let index = block_index(span, local)?;
            if index >= output.len() || index >= live.len() {
                return Err(PlaybackError::BufferTooSmall);
            }
            output[index] = live[index];
        }
    }
    Ok(())
}

fn read_cache(
    samples: &[f32],
    origin: i64,
    spans: &[PlayedSpan],
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    for span in spans {
        for local in 0..span.frames {
            let index = block_index(span, local)?;
            if index >= output.len() {
                return Err(PlaybackError::BufferTooSmall);
            }
            let project = span
                .start_sample
                .checked_add(i64::from(local))
                .ok_or(PlaybackError::Time)?;
            output[index] = cached_sample(samples, origin, project);
        }
    }
    Ok(())
}

fn cached_sample(samples: &[f32], origin: i64, project: i64) -> f32 {
    let Some(index) = project.checked_sub(origin) else {
        return 0.0;
    };
    if index < 0 {
        return 0.0;
    }
    let Some(index) = usize::try_from(index).ok() else {
        return 0.0;
    };
    samples.get(index).copied().unwrap_or(0.0)
}

fn clip_sample(clip: &AudioClip<'_>, project: i64, fade: i64) -> f32 {
    let Some(pos) = project.checked_sub(clip.start_sample) else {
        return 0.0;
    };
    if pos < 0 {
        return 0.0;
    }
    let Some(index) = usize::try_from(pos).ok() else {
        return 0.0;
    };
    let Some(sample) = clip.samples.get(index).copied() else {
        return 0.0;
    };
    sample * edge_gain(pos, clip.samples.len() as i64, fade)
}

fn edge_gain(pos: i64, length: i64, fade: i64) -> f32 {
    if length <= 1 || fade <= 1 {
        return if pos == 0 || pos == length - 1 {
            0.0
        } else {
            1.0
        };
    }
    let fade = fade.min(length);
    let denom = (fade - 1) as f32;
    let mut gain: f32 = 1.0;
    if pos < fade {
        gain = gain.min(pos as f32 / denom);
    }
    let tail = length - 1 - pos;
    if tail < fade {
        gain = gain.min(tail as f32 / denom);
    }
    gain
}

fn block_index(span: &PlayedSpan, local: u32) -> Result<usize, PlaybackError> {
    span.block_offset
        .checked_add(local)
        .map(|index| index as usize)
        .ok_or(PlaybackError::Time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: i64, frames: u32, offset: u32, release_at: Option<i64>) -> PlayedSpan {
        PlayedSpan {
            start_sample: start,
            frames,
            block_offset: offset,
            release_at,
            loop_end: None,
            wrap_from: None,
        }
    }

    #[test]
    fn a_track_can_switch_from_live_devices_to_a_cache() {
        let live = [0.25, 0.5, 0.75, 1.0];
        let spans = [span(10, 4, 0, None)];
        let mut from_devices = [0.0; 4];
        render_track(TrackSource::Live, &live, &spans, &mut from_devices).unwrap();
        assert_eq!(from_devices, live);

        let cached = [9.0, 8.0, 7.0, 6.0, 5.0];
        let mut from_cache = [0.0; 4];
        render_track(
            TrackSource::Cache {
                samples: &cached,
                origin: 8,
            },
            &live,
            &spans,
            &mut from_cache,
        )
        .unwrap();
        assert_eq!(from_cache, [7.0, 6.0, 5.0, 0.0]);
    }

    #[test]
    fn a_wrapped_block_reads_the_loop_head_from_the_cache() {
        let cached = [1.0, 2.0, 3.0, 4.0];
        let spans = [span(2, 2, 0, None), span(0, 2, 2, Some(4))];
        let mut output = [0.0; 4];
        render_track(
            TrackSource::Cache {
                samples: &cached,
                origin: 0,
            },
            &[],
            &spans,
            &mut output,
        )
        .unwrap();
        assert_eq!(output, [3.0, 4.0, 1.0, 2.0]);
    }

    #[test]
    fn the_loop_head_is_requested_only_as_the_end_approaches() {
        assert_eq!(loop_head_prefetch(0, 0, 1_000, 64, 240), None);
        assert_eq!(
            loop_head_prefetch(950, 0, 1_000, 64, 240),
            Some(LoopHead {
                start_sample: 0,
                frames: 240,
            })
        );
        assert_eq!(loop_head_prefetch(1_000, 0, 1_000, 64, 240), None);
    }

    #[test]
    fn clip_edges_and_the_loop_jump_fade_over_five_milliseconds() {
        assert_eq!(seam_fade_frames(1_000), 5);
        let ones = [1.0; 20];
        let clip = AudioClip {
            start_sample: 0,
            samples: &ones,
        };
        let mut output = [0.0; 20];
        render_audio_clips(&[clip], &[span(0, 20, 0, None)], 1_000, &mut output).unwrap();
        assert_eq!(output[0], 0.0);
        assert_eq!(output[4], 1.0);
        assert_eq!(output[10], 1.0);
        assert_eq!(output[15], 1.0);
        assert_eq!(output[19], 0.0);
        let faded = FadedClip {
            start_sample: 0,
            samples: &ones,
            fade_in: 5,
            fade_out: 5,
        };
        let mut matched = [0.0; 20];
        render_faded_clips(&[faded], &[span(0, 20, 0, None)], &mut matched).unwrap();
        assert_eq!(matched, output);
        let full = FadedClip {
            start_sample: 0,
            samples: &ones,
            fade_in: 0,
            fade_out: 0,
        };
        let mut open = [0.0; 20];
        render_faded_clips(&[full], &[span(0, 20, 0, None)], &mut open).unwrap();
        assert_eq!(open[0], 1.0);
        assert_eq!(open[19], 1.0);

        let mut seam = [1.0; 20];
        let mut outgoing = span(0, 10, 0, None);
        outgoing.loop_end = Some(10);
        let mut incoming = span(0, 10, 10, Some(10));
        incoming.wrap_from = Some(0);
        fade_loop_seam(&[outgoing, incoming], 5, &mut seam).unwrap();
        assert_eq!(seam[4], 1.0);
        assert_eq!(seam[5], 1.0);
        assert_eq!(seam[9], 0.0);
        assert_eq!(seam[10], 0.0);
        assert_eq!(seam[14], 1.0);
        assert_eq!(seam[15], 1.0);
    }

    #[test]
    fn a_block_that_crosses_two_clips_plays_both() {
        let head = [1.0; 4];
        let tail = [1.0; 4];
        let clips = [
            AudioClip {
                start_sample: 0,
                samples: &head,
            },
            AudioClip {
                start_sample: 4,
                samples: &tail,
            },
        ];
        let mut output = [0.0; 8];
        render_audio_clips(&clips, &[span(0, 8, 0, None)], 1, &mut output).unwrap();
        assert_eq!(output[1], 1.0);
        assert_eq!(output[4], 0.0);
        assert_eq!(output[5], 1.0);
    }
}
