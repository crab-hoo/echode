//! Offline render of a project through the test synth.
//!
//! Each block uses a fixed scratch length. That length is not a limit on the
//! render. Voice slots follow the notes in the project, not a polyphony cap.
//! Scheduling and the synth do not allocate. Gathering the track slices for
//! the mixer does, and this function is not called from the audio thread.

use echode_model::{Id, Project, Tick};

use crate::clock::tick_to_sample;
use crate::mix::{TrackFader, mix_tracks};
use crate::realtime::ForbidAlloc;
use crate::sequence::{MidiEvent, MidiKind, schedule};
use crate::synth::{SynthVoice, render_test_synth};
use crate::transport::{PlaybackError, PlayedSpan, Transport};

/// Scratch length for one offline block. The render can be any number of frames.
const BLOCK_FRAMES: u32 = 64;

/// wavesynth 0.1.0 Sine Pad release, in milliseconds.
const SINE_PAD_RELEASE_MS: u64 = 400;

/// How many frames cover the score plus the Sine Pad release.
pub fn render_span_frames(project: &Project, sample_rate: u32) -> Result<u32, PlaybackError> {
    if sample_rate == 0 {
        return Err(PlaybackError::InvalidSampleRate);
    }
    let mut end = 0i64;
    for track in project.tracks() {
        for clip in &track.clips {
            for note in &clip.notes {
                let start = clip.absolute_start(note).ok_or(PlaybackError::Time)?;
                let end_tick = start
                    .get()
                    .checked_add(note.length.get())
                    .ok_or(PlaybackError::Time)?;
                let sample = tick_to_sample(Tick::new(end_tick), project.tempo(), sample_rate)
                    .ok_or(PlaybackError::Time)?;
                end = end.max(sample);
            }
        }
    }
    if end == 0
        && project
            .tracks()
            .iter()
            .all(|track| track.clips.iter().all(|clip| clip.notes.is_empty()))
    {
        return Ok(0);
    }
    let release = u64::from(sample_rate) * SINE_PAD_RELEASE_MS / 1_000;
    let release = u32::try_from(release).map_err(|_| PlaybackError::Time)?;
    u32::try_from(end.max(0))
        .map_err(|_| PlaybackError::Time)?
        .checked_add(release)
        .ok_or(PlaybackError::Time)
}

/// Renders `frames` of stereo, interleaved left then right.
///
/// Every track is the test synth at unity gain, centered. The buffer length
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
    let mut transport = Transport::new(sample_rate, project.tempo().clone())?;
    transport.play();

    let tracks = project.tracks();
    let note_counts: Vec<usize> = tracks
        .iter()
        .map(|track| track.clips.iter().map(|clip| clip.notes.len()).sum())
        .collect();
    let mut voices = Vec::with_capacity(tracks.len());
    for count in &note_counts {
        let mut track_voices = Vec::with_capacity((*count).max(1));
        for _ in 0..(*count).max(1) {
            track_voices.push(SynthVoice::new(sample_rate)?);
        }
        voices.push(track_voices);
    }
    let event_cap = note_counts.iter().sum::<usize>().saturating_mul(4).max(1);
    let silent = MidiEvent {
        sample_offset: 0,
        track: Id::from_raw(0),
        note: Id::from_raw(0),
        pitch: 0,
        velocity: 0,
        kind: MidiKind::Off,
    };
    let mut events = vec![silent; event_cap];
    let mut track_events = vec![silent; event_cap];
    let mut planes = vec![vec![0.0; BLOCK_FRAMES as usize]; tracks.len()];
    let mut left = vec![0.0; BLOCK_FRAMES as usize];
    let mut right = vec![0.0; BLOCK_FRAMES as usize];
    let mut peaks = vec![0.0; tracks.len()];
    let faders = vec![TrackFader::unity(); tracks.len()];
    let mut spans = [PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
    }; 2];

    let mut rendered = 0u32;
    while rendered < frames {
        let block = (frames - rendered).min(BLOCK_FRAMES);
        let block_len = block as usize;
        {
            let _guard = ForbidAlloc::enter();
            let span_count = transport.pull(block, &mut spans)?;
            let event_count = schedule(
                tracks,
                project.tempo(),
                sample_rate,
                &spans[..span_count],
                &mut events,
            )?;
            for (track_index, track) in tracks.iter().enumerate() {
                planes[track_index][..block_len].fill(0.0);
                let mut kept = 0;
                for event in &events[..event_count] {
                    if event.track == track.id {
                        track_events[kept] = *event;
                        kept += 1;
                    }
                }
                render_test_synth(
                    &track_events[..kept],
                    &mut voices[track_index],
                    sample_rate,
                    &mut planes[track_index][..block_len],
                )?;
            }
        }
        let plane_refs: Vec<&[f32]> = planes.iter().map(|plane| &plane[..block_len]).collect();
        mix_tracks(
            &plane_refs,
            &faders,
            &mut left[..block_len],
            &mut right[..block_len],
            &mut peaks,
        )?;
        for index in 0..block_len {
            let start = (rendered as usize + index) * 2;
            interleaved[start] = left[index];
            interleaved[start + 1] = right[index];
        }
        rendered += block;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_model::{NoteDraft, Project, Tick};

    fn noted() -> Project {
        let (project, track) = Project::new().add_track("piano");
        project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 69,
                    velocity: 127,
                    start: Tick::new(0),
                    length: Tick::quarters(1).unwrap(),
                },
            )
            .unwrap()
            .0
    }

    #[test]
    fn two_renders_match_and_a_note_is_audible() {
        let project = noted();
        let mut once = vec![0.0; 256];
        let mut twice = vec![0.0; 256];
        render_offline(&project, 48_000, 128, &mut once).unwrap();
        render_offline(&project, 48_000, 128, &mut twice).unwrap();
        assert_eq!(once, twice);
        assert_eq!(once[0], 0.0);
        assert!(once.iter().any(|sample| *sample != 0.0));
    }

    #[test]
    fn the_default_length_includes_the_release() {
        let project = noted();
        assert_eq!(
            render_span_frames(&project, 48_000).unwrap(),
            24_000 + 19_200
        );
        assert_eq!(render_span_frames(&Project::new(), 48_000).unwrap(), 0);
        let mut silence = [];
        render_offline(&Project::new(), 48_000, 0, &mut silence).unwrap();
    }
}
