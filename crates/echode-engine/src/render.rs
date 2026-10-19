//! Offline export through product playback, with the same clip buses and fader.
//! Only preparation and the writer callback may allocate.

use crate::builtin::Device;
use echode_model::Project;

use crate::clock::tick_to_sample;
use crate::synth::TestSynth;
use crate::transport::PlaybackError;

/// Scratch length for one offline block. The render can be any number of frames.
const BLOCK_FRAMES: u32 = 64;

/// How many frames cover the score.
///
/// A note's end includes the test synth's tail. An audio clip ends at the
/// placement, with no extra tail.
pub fn render_span_frames(project: &Project, sample_rate: u32) -> Result<u32, PlaybackError> {
    if sample_rate == 0 {
        return Err(PlaybackError::InvalidSampleRate);
    }
    let mut note_end = 0i64;
    let mut audio_end = 0i64;
    let mut heard_notes = false;
    for track in project.tracks() {
        for clip in &track.clips {
            let Some(content) = project.content(clip.content) else {
                return Err(PlaybackError::Time);
            };
            if content.audio.is_some() {
                let end_tick = clip
                    .start
                    .get()
                    .checked_add(clip.length.get())
                    .ok_or(PlaybackError::Time)?;
                let sample = tick_to_sample(
                    echode_model::Tick::new(end_tick),
                    project.tempo(),
                    sample_rate,
                )
                .ok_or(PlaybackError::Time)?;
                audio_end = audio_end.max(sample);
            }
            for note in &content.notes {
                let Some((_, off)) = clip.audible(note).map_err(|_| PlaybackError::Time)? else {
                    continue;
                };
                let sample =
                    tick_to_sample(off, project.tempo(), sample_rate).ok_or(PlaybackError::Time)?;
                heard_notes = true;
                note_end = note_end.max(sample);
            }
        }
    }
    if !heard_notes && audio_end == 0 {
        return Ok(0);
    }
    let mut end = audio_end.max(0);
    if heard_notes {
        let release = i64::from(TestSynth::new(sample_rate)?.tail_samples(sample_rate));
        end = end.max(note_end.saturating_add(release));
    }
    u32::try_from(end).map_err(|_| PlaybackError::Time)
}

/// Why a block-wise offline render stopped.
#[derive(Debug)]
pub enum OfflineError<E> {
    Playback(PlaybackError),
    Write(E),
}

impl<E: std::fmt::Display> std::fmt::Display for OfflineError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Playback(error) => write!(formatter, "{error}"),
            Self::Write(error) => write!(formatter, "{error}"),
        }
    }
}

/// Renders `frames` of stereo, interleaved left then right.
///
/// Uses each track's test synth and project fader values. The buffer length
/// must be `frames * 2`.
pub fn render_offline(
    project: &Project,
    sample_rate: u32,
    frames: u32,
    interleaved: &mut [f32],
) -> Result<(), PlaybackError> {
    let frame_count = usize::try_from(frames).map_err(|_| PlaybackError::Time)?;
    let needed = frame_count.checked_mul(2).ok_or(PlaybackError::Time)?;
    if interleaved.len() != needed {
        return Err(PlaybackError::BufferTooSmall);
    }
    let mut written = 0usize;
    render_offline_blocks(project, sample_rate, frames, BLOCK_FRAMES, |block| {
        let end = written + block.len();
        interleaved[written..end].copy_from_slice(block);
        written += block.len();
        Ok(())
    })
    .map_err(|error| match error {
        OfflineError::Playback(error) => error,
        OfflineError::Write(error) => error,
    })
}

/// Renders `frames` of stereo and calls `on_block` with each interleaved block.
///
/// `block_frames` is scratch for one block, not a limit on `frames`. `on_block`
/// sees left then right for that block only, so a long export can write it
/// before the next block is rendered. Placed audio is not included; pass it to
/// [`render_offline_blocks_with_clips`].
pub fn render_offline_blocks<E>(
    project: &Project,
    sample_rate: u32,
    frames: u32,
    block_frames: u32,
    on_block: impl FnMut(&[f32]) -> Result<(), E>,
) -> Result<(), OfflineError<E>> {
    render_offline_blocks_with_clips(project, sample_rate, frames, block_frames, &[], on_block)
}

/// [`render_offline_blocks`] plus device-rate clips mixed after the synth and
/// before the fader. This path does not host CLAP devices. `echode render`
/// uses product playback for a project that has one.
pub fn render_offline_blocks_with_clips<E>(
    project: &Project,
    sample_rate: u32,
    frames: u32,
    block_frames: u32,
    clips: &[crate::playback::LoadedClip],
    mut on_block: impl FnMut(&[f32]) -> Result<(), E>,
) -> Result<(), OfflineError<E>> {
    if block_frames == 0 {
        return Err(OfflineError::Playback(PlaybackError::BufferTooSmall));
    }
    let scratch = block_frames as usize;
    let (mut control, mut audio) = crate::playback::playback_pair(project, sample_rate, scratch)
        .map_err(OfflineError::Playback)?;
    let clips = clips.to_vec();
    control.set_clip_loader(std::sync::Arc::new(move |_, _| Ok(clips.clone())));
    control.replace(project).map_err(OfflineError::Playback)?;
    control
        .command(crate::PlaybackCommand::Play)
        .map_err(OfflineError::Playback)?;
    let mut left = vec![0.0; scratch];
    let mut right = vec![0.0; scratch];
    let mut interleaved = vec![0.0; scratch * 2];
    let mut rendered = 0u32;
    while rendered < frames {
        let count = (frames - rendered).min(block_frames) as usize;
        audio
            .render_checked(
                &mut left[..count],
                &mut right[..count],
                std::time::Instant::now(),
            )
            .map_err(OfflineError::Playback)?;
        for (pair, (left, right)) in interleaved
            .chunks_exact_mut(2)
            .zip(left.iter().zip(&right))
            .take(count)
        {
            pair.copy_from_slice(&[*left, *right]);
        }
        on_block(&interleaved[..count * 2]).map_err(OfflineError::Write)?;
        rendered += count as u32;
    }
    Ok(())
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
