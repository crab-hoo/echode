//! Offline render of a project through the test synth.
//!
//! Each block uses a fixed scratch length. That length is not a limit on the
//! render. Voice slots follow the deepest overlap, including release, not the
//! length of the score. Scheduling and the synth do not allocate. Gathering the track
//! slices for the mixer does, and this function is not called from the audio thread.

use echode_model::Project;

use crate::builtin::Device;
use crate::clock::tick_to_sample;
use crate::mix::{TrackFader, mix_tracks};
use crate::plan::PlaybackIds;
use crate::realtime::ForbidAlloc;
use crate::sequence::play_prepared;
use crate::synth::TestSynth;
use crate::transport::{PlaybackError, PlayedSpan, Transport};
use echode_realtime::{Event, sort_events};

/// Scratch length for one offline block. The render can be any number of frames.
const BLOCK_FRAMES: u32 = 64;

fn track_fader(track: &echode_model::Track) -> Result<TrackFader, PlaybackError> {
    use echode_model::{DeviceKind, ParamKey};
    let mixer = track
        .devices
        .iter()
        .find(|device| device.kind == DeviceKind::Mixer)
        .ok_or(PlaybackError::InvalidFader)?;
    let value = |key| {
        mixer
            .value(key)
            .filter(|value| key.allows(*value))
            .ok_or(PlaybackError::InvalidFader)
    };
    Ok(TrackFader {
        gain: value(ParamKey::Gain)?,
        pan: value(ParamKey::Pan)?,
        mute: value(ParamKey::Mute)? != 0.0,
        solo: value(ParamKey::Solo)? != 0.0,
    })
}

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
    let mut transport =
        Transport::new(sample_rate, project.tempo().clone()).map_err(OfflineError::Playback)?;
    transport.play();

    let tracks = project.tracks();
    let mut ids = PlaybackIds::new();
    let plan = crate::plan::build_plan(project, project.tempo(), sample_rate, &mut ids)
        .map_err(OfflineError::Playback)?;
    let tail = i64::from(
        TestSynth::new(sample_rate)
            .map_err(OfflineError::Playback)?
            .tail_samples(sample_rate),
    );
    let mut devices = Vec::with_capacity(plan.tracks.len());
    let mut note_total = 0usize;
    for track in &plan.tracks {
        note_total += track.notes.len();
        let mut device = TestSynth::new(sample_rate).map_err(OfflineError::Playback)?;
        let voices = crate::plan::voice_demand(&track.notes, tail).max(1);
        device
            .prepare(voices, sample_rate)
            .map_err(OfflineError::Playback)?;
        devices.push(device);
    }
    let event_cap = note_total.saturating_mul(4).saturating_add(2).max(1);
    let silent = Event::note_off(0, 0, 0, 0);
    let mut events = vec![silent; event_cap];
    let mut track_events = vec![silent; event_cap];
    let scratch = block_frames as usize;
    let mut planes = vec![vec![0.0; scratch]; tracks.len()];
    let mut left = vec![0.0; scratch];
    let mut right = vec![0.0; scratch];
    let mut interleaved_block = vec![0.0; scratch * 2];
    let mut peaks = vec![0.0; tracks.len()];
    let faders = tracks
        .iter()
        .map(|track| track_fader(track))
        .collect::<Result<Vec<_>, _>>()
        .map_err(OfflineError::Playback)?;
    let mut spans = [PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }; 2];
    let mut cursor = plan.cursor();
    let meter = project.meter();

    let mut rendered = 0u32;
    while rendered < frames {
        let block = (frames - rendered).min(block_frames);
        let block_len = block as usize;
        let span_count = {
            let _guard = ForbidAlloc::enter();
            let span_count = transport
                .pull(block, &mut spans)
                .map_err(OfflineError::Playback)?;
            let mut event_count =
                play_prepared(&plan, &mut cursor, &spans[..span_count], &mut events)
                    .map_err(OfflineError::Playback)?;
            for span in &spans[..span_count] {
                if event_count >= events.len() {
                    return Err(OfflineError::Playback(PlaybackError::BufferTooSmall));
                }
                events[event_count] =
                    transport.transport_event(span.start_sample, span.block_offset, meter);
                event_count += 1;
            }
            sort_events(&mut events[..event_count]);
            for (track_index, _track) in tracks.iter().enumerate() {
                planes[track_index][..block_len].fill(0.0);
                let mut kept = 0;
                for event in &events[..event_count] {
                    let keep = match event.track_key() {
                        None => true,
                        Some(key) => key == track_index as u32,
                    };
                    if keep {
                        track_events[kept] = *event;
                        kept += 1;
                    }
                }
                let mut channels = [&mut planes[track_index][..block_len]];
                devices[track_index]
                    .process(&mut channels, &track_events[..kept], sample_rate)
                    .map_err(OfflineError::Playback)?;
            }
            span_count
        };
        add_offline_clips(tracks, clips, &spans[..span_count], &mut planes, block_len)
            .map_err(OfflineError::Playback)?;
        let plane_refs: Vec<&[f32]> = planes.iter().map(|plane| &plane[..block_len]).collect();
        mix_tracks(
            &plane_refs,
            &faders,
            &mut left[..block_len],
            &mut right[..block_len],
            &mut peaks,
        )
        .map_err(OfflineError::Playback)?;
        for index in 0..block_len {
            interleaved_block[index * 2] = left[index];
            interleaved_block[index * 2 + 1] = right[index];
        }
        on_block(&interleaved_block[..block_len * 2]).map_err(OfflineError::Write)?;
        rendered += block;
    }
    Ok(())
}

fn add_offline_clips(
    tracks: &[std::sync::Arc<echode_model::Track>],
    clips: &[crate::playback::LoadedClip],
    spans: &[PlayedSpan],
    planes: &mut [Vec<f32>],
    block_len: usize,
) -> Result<(), PlaybackError> {
    if clips.is_empty() || spans.is_empty() {
        return Ok(());
    }
    for clip in clips {
        let track_index = tracks
            .iter()
            .position(|track| track.id.raw() == clip.track)
            .ok_or(PlaybackError::Time)?;
        let view = [crate::voice::FadedClip {
            start_sample: clip.start_sample,
            samples: clip.samples.as_ref(),
            fade_in: clip.fade_in,
            fade_out: clip.fade_out,
        }];
        let plane = planes.get_mut(track_index).ok_or(PlaybackError::Time)?;
        crate::voice::render_faded_clips(&view, spans, &mut plane[..block_len])?;
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
        let mut streamed = Vec::new();
        render_offline_blocks(&project, 48_000, 128, BLOCK_FRAMES, |block| {
            streamed.extend_from_slice(block);
            Ok::<(), PlaybackError>(())
        })
        .unwrap();
        assert_eq!(once, streamed);
    }

    #[test]
    fn a_silent_fader_renders_silence() {
        let project = noted();
        let gain = project.tracks()[0].devices[0]
            .parameters
            .iter()
            .find(|parameter| parameter.key == echode_model::ParamKey::Gain)
            .unwrap()
            .id;
        let project = project.set_parameter(gain, 0.0).unwrap();
        let mut samples = vec![0.0; 256];
        render_offline(&project, 48_000, 128, &mut samples).unwrap();
        assert!(samples.iter().all(|sample| *sample == 0.0));
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

    #[test]
    fn an_audio_clip_sets_the_length_without_the_synth_tail() {
        use echode_model::{AudioRef, Id};
        let (project, track) = Project::new().add_track("file");
        let audio = AudioRef::try_new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            48_000,
            1,
            48_000,
        )
        .unwrap();
        let project = project
            .place_audio(track, Id::generate(), Id::generate(), Tick::new(0), audio)
            .unwrap();
        let clip = &project.tracks()[0].clips[0];
        let end = tick_to_sample(
            Tick::new(clip.start.get() + clip.length.get()),
            project.tempo(),
            48_000,
        )
        .unwrap();
        assert!(end > 0);
        assert_eq!(
            render_span_frames(&project, 48_000).unwrap(),
            u32::try_from(end).unwrap()
        );
    }

    #[test]
    fn offline_clips_match_playback() {
        use echode_model::{AudioRef, Id};
        use std::sync::Arc;
        let (project, track) = Project::new().add_track("audio");
        let audio = AudioRef::try_new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            48_000,
            1,
            64,
        )
        .unwrap();
        let project = project
            .place_audio(track, Id::generate(), Id::generate(), Tick::new(0), audio)
            .unwrap();
        let samples: Arc<[f32]> = (0..64).map(|index| index as f32 / 64.0).collect();
        let loaded = vec![crate::playback::LoadedClip {
            track: project.tracks()[0].id.raw(),
            start_sample: 0,
            samples: Arc::clone(&samples),
            fade_in: 8,
            fade_out: 8,
        }];
        let mut offline = Vec::new();
        render_offline_blocks_with_clips(&project, 48_000, 200, 64, &loaded, |block| {
            offline.extend_from_slice(block);
            Ok::<(), PlaybackError>(())
        })
        .unwrap();
        let (mut control, mut playback) =
            crate::playback::playback_pair(&project, 48_000, 64).unwrap();
        let samples = Arc::clone(&samples);
        let track_raw = project.tracks()[0].id.raw();
        control.set_clip_loader(Arc::new(move |_project, _rate| {
            Ok(vec![crate::playback::LoadedClip {
                track: track_raw,
                start_sample: 0,
                samples: Arc::clone(&samples),
                fade_in: 8,
                fade_out: 8,
            }])
        }));
        control.replace(&project).unwrap();
        control
            .command(crate::playback::PlaybackCommand::Play)
            .unwrap();
        let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
        let mut heard = Vec::with_capacity(400);
        let mut remaining = 200usize;
        while remaining > 0 {
            let count = remaining.min(64);
            playback.render(&mut left[..count], &mut right[..count]);
            for (left, right) in left.iter().zip(&right).take(count) {
                heard.extend([*left, *right]);
            }
            remaining -= count;
        }
        assert!(!control.status().failed);
        assert!(offline.iter().any(|sample| *sample != 0.0));
        assert_eq!(offline, heard);
    }

    fn crowded(extra: usize) -> Project {
        use echode_model::{
            ClipKind, Content, DEFAULT_FADE_FLICKS, Id, Lane, Meter, Note, Placement,
            TICKS_PER_QUARTER, Tempo, TempoMap, Track,
        };
        let mut notes = Vec::with_capacity(80 + extra);
        for index in 0..80 {
            notes.push(Note {
                id: Id::from_raw(100_000 + index as u128),
                start: Tick::new((index as i64 / 4) * TICKS_PER_QUARTER),
                length: Tick::quarters(1).unwrap(),
                pitch: 60 + (index % 12) as u8,
                velocity: echode_model::midi7_to_velocity(100),
                release: 0,
                articulation: None,
            });
        }
        let far = 10_000i64 * TICKS_PER_QUARTER;
        for index in 0..extra {
            notes.push(Note {
                id: Id::from_raw(200_000 + index as u128),
                start: Tick::new(far + index as i64 * TICKS_PER_QUARTER),
                length: Tick::quarters(1).unwrap(),
                pitch: 60,
                velocity: echode_model::midi7_to_velocity(100),
                release: 0,
                articulation: None,
            });
        }
        let end = far + (extra as i64 + 1) * TICKS_PER_QUARTER;
        Project::try_assemble(
            Meter::four_four(),
            TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
            vec![Content {
                id: Id::from_raw(3),
                audio: None,
                takes: Vec::new(),
                expressions: Vec::new(),
                notes,
            }],
            vec![Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                order: echode_model::initial_order(),
                devices: vec![echode_model::test_mixer(1)],
                clips: vec![Placement {
                    id: Id::from_raw(2),
                    content: Id::from_raw(3),
                    lane: Lane::Timeline,
                    start: Tick::new(0),
                    length: Tick::new(end),
                    offset: Tick::new(0),
                    kind: ClipKind::Fixed,
                    gain: 1.0,
                    fade_in: DEFAULT_FADE_FLICKS,
                    fade_out: DEFAULT_FADE_FLICKS,
                }],
            }],
        )
        .expect("assemble")
    }

    #[test]
    fn notes_outside_the_render_do_not_change_the_sound() {
        let mut small = vec![0.0; 8_192];
        let mut large = vec![0.0; 8_192];
        render_offline(&crowded(0), 48_000, 4_096, &mut small).unwrap();
        render_offline(&crowded(500), 48_000, 4_096, &mut large).unwrap();
        assert_eq!(small, large);
    }

    #[test]
    #[ignore = "timing measurement; `make spike-engine` runs it in a release build"]
    fn offline_render_time_follows_the_sounding_notes() {
        const FRAMES: u32 = 48_000 * 10;
        let small = crowded(0);
        let large = crowded(3_920);
        let mut buffer = vec![0.0; FRAMES as usize * 2];
        let started = std::time::Instant::now();
        render_offline(&small, 48_000, FRAMES, &mut buffer).unwrap();
        let small_time = started.elapsed();
        let started = std::time::Instant::now();
        render_offline(&large, 48_000, FRAMES, &mut buffer).unwrap();
        let large_time = started.elapsed();
        println!(
            "10s render, 80 sounding notes: {small_time:?}; same window inside 4,000 notes: {large_time:?}"
        );
        let ratio = large_time.as_secs_f64() / small_time.as_secs_f64().max(1.0e-6);
        assert!(
            ratio < 3.0,
            "render time grew {ratio:.2}× with notes that do not sound"
        );
    }
}
