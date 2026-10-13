//! Output must not depend on how the timeline is cut into blocks.
//!
//! Offline export uses one block length, a device uses another, and the preview
//! cache must match live playback (ARCHITECTURE §7.8). Each test renders the
//! same material with several block lengths and compares the samples bit for bit.
//! The block lengths and voice slots are this test's choice, not engine limits.

use echode_engine::{
    AudioClip, Event, PlayedSpan, SynthVoice, TrackFader, Transport, fade_loop_seam, mix_tracks,
    render_audio_clips, render_metronome, render_test_synth, schedule, seam_fade_frames,
};
use echode_model::{
    AudioRef, Id, Meter, NoteDraft, Project, TICKS_PER_QUARTER, Tempo, TempoMap, Tick,
};
use std::sync::Arc;

const RATE: u32 = 48_000;
const BLOCKS: [u32; 4] = [1, 7, 64, 512];
const VOICES_PER_TRACK: usize = 8;

#[test]
fn product_playback_matches_offline_export_for_every_block_size() {
    let project = project();
    let frames = 100000u32;
    let mut expected = vec![0.0; frames as usize * 2];
    echode_engine::render_offline(&project, RATE, frames, &mut expected).unwrap();
    for block in BLOCKS {
        let (mut control, mut audio) =
            echode_engine::playback_pair(&project, RATE, block as usize).unwrap();
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        let mut output = Vec::with_capacity(expected.len());
        let mut remaining = frames;
        while remaining > 0 {
            let count = remaining.min(block) as usize;
            audio.render(&mut left[..count], &mut right[..count]);
            for (left, right) in left.iter().zip(&right).take(count) {
                output.extend([*left, *right]);
            }
            remaining -= count as u32;
        }
        assert!(!control.status().failed);
        assert_eq!(output, expected, "product block {block}");
    }
}

#[test]
fn product_loop_and_metronome_match_across_block_sizes() {
    let project = project();
    let end =
        echode_engine::tick_to_sample(Tick::new(TICKS_PER_QUARTER), project.tempo(), RATE).unwrap();
    let frames = 48_000u32;
    let mut reference: Option<Vec<f32>> = None;
    for block in BLOCKS {
        let (mut control, mut audio) =
            echode_engine::playback_pair(&project, RATE, block as usize).unwrap();
        control
            .command(echode_engine::PlaybackCommand::SetLoop { start: 0, end })
            .unwrap();
        control
            .command(echode_engine::PlaybackCommand::SetMetronome(true))
            .unwrap();
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        let mut output = Vec::with_capacity(frames as usize * 2);
        let mut remaining = frames;
        while remaining > 0 {
            let count = remaining.min(block) as usize;
            audio.render(&mut left[..count], &mut right[..count]);
            for (left, right) in left.iter().zip(&right).take(count) {
                output.extend([*left, *right]);
            }
            remaining -= count as u32;
        }
        assert!(!control.status().failed);
        assert!(control.status().sample <= end);
        assert!(output.iter().any(|sample| *sample != 0.0));
        if let Some(reference) = &reference {
            assert_eq!(&output, reference, "loop block {block}");
        } else {
            reference = Some(output);
        }
    }
}

#[test]
fn product_audio_clips_match_across_block_sizes() {
    let (project, track) = Project::new().add_track("audio");
    let audio = AudioRef::try_new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        RATE,
        1,
        64,
    )
    .unwrap();
    let project = project
        .place_audio(track, Id::generate(), Id::generate(), Tick::new(0), audio)
        .unwrap();
    let samples: Arc<[f32]> = (0..64).map(|index| index as f32 / 64.0).collect();
    let track_raw = project.tracks()[0].id.raw();
    let frames = 200u32;
    let mut reference: Option<Vec<f32>> = None;
    for block in BLOCKS {
        let (mut control, mut audio) =
            echode_engine::playback_pair(&project, RATE, block as usize).unwrap();
        let samples = Arc::clone(&samples);
        control.set_clip_loader(Arc::new(move |_project, _rate| {
            Ok(vec![echode_engine::LoadedClip {
                track: track_raw,
                start_sample: 0,
                samples: Arc::clone(&samples),
                fade_in: 8,
                fade_out: 8,
            }])
        }));
        control.replace(&project).unwrap();
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        let mut output = Vec::with_capacity(frames as usize * 2);
        let mut remaining = frames;
        while remaining > 0 {
            let count = remaining.min(block) as usize;
            audio.render(&mut left[..count], &mut right[..count]);
            for (left, right) in left.iter().zip(&right).take(count) {
                output.extend([*left, *right]);
            }
            remaining -= count as u32;
        }
        assert!(!control.status().failed);
        assert!(output.iter().any(|sample| *sample != 0.0));
        if let Some(reference) = &reference {
            assert_eq!(&output, reference, "audio clip block {block}");
        } else {
            reference = Some(output);
        }
    }
}

fn tempo() -> TempoMap {
    TempoMap::new(Tempo::quarters_per_minute(120).unwrap())
}

fn quarters(value: f64) -> Tick {
    Tick::new((value * TICKS_PER_QUARTER as f64) as i64)
}

fn note(start: f64, length: f64, pitch: u8) -> NoteDraft {
    NoteDraft {
        pitch,
        velocity: 100,
        start: quarters(start),
        length: quarters(length),
    }
}

/// Two tracks with overlapping notes; one note runs across the one-quarter loop end.
fn project() -> Project {
    let (project, lead) = Project::new().add_track("lead");
    let (project, bass) = project.add_track("bass");
    let mut project = project;
    for draft in [note(0.0, 1.0, 60), note(0.5, 2.0, 64), note(3.0, 1.0, 67)] {
        project = project.insert_note(lead, draft).unwrap().0;
    }
    for draft in [note(0.25, 0.25, 48), note(0.75, 1.5, 52)] {
        project = project.insert_note(bass, draft).unwrap().0;
    }
    project
}

fn empty_span() -> PlayedSpan {
    PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }
}

fn silent_event() -> Event {
    Event::note_off(0, 0, 0, 0)
}

/// Pulls `frames` in blocks of `block` and hands each block's spans and length to `render`.
fn drive(
    frames: u32,
    block: u32,
    looped: Option<(Tick, Tick)>,
    mut render: impl FnMut(&[PlayedSpan], usize, &mut Vec<f32>),
) -> Vec<f32> {
    let mut transport = Transport::new(RATE, tempo()).unwrap();
    transport.set_loop(looped).unwrap();
    transport.play();
    let mut spans = [empty_span(); 4];
    let mut out = Vec::new();
    let mut done = 0;
    while done < frames {
        let length = (frames - done).min(block);
        let count = transport.pull(length, &mut spans).unwrap();
        render(&spans[..count], length as usize, &mut out);
        done += length;
    }
    out
}

/// The test synth on every track, mixed to stereo at unity.
fn render_synth(
    project: &Project,
    frames: u32,
    block: u32,
    looped: Option<(Tick, Tick)>,
) -> Vec<f32> {
    let tracks = project.tracks();
    let mut voices: Vec<Vec<SynthVoice>> = tracks
        .iter()
        .map(|_| {
            (0..VOICES_PER_TRACK)
                .map(|_| SynthVoice::new(RATE).unwrap())
                .collect()
        })
        .collect();
    let mut events = vec![silent_event(); 256];
    let mut track_events = vec![silent_event(); 256];
    let mut planes = vec![vec![0.0f32; block as usize]; tracks.len()];
    let faders = vec![TrackFader::unity(); tracks.len()];
    let mut peaks = vec![0.0f32; tracks.len()];
    let mut left = vec![0.0f32; block as usize];
    let mut right = vec![0.0f32; block as usize];
    drive(frames, block, looped, |spans, length, out| {
        let count = schedule(project, project.tempo(), RATE, spans, &mut events).unwrap();
        for (index, _track) in tracks.iter().enumerate() {
            let mut kept = 0;
            for event in events[..count]
                .iter()
                .filter(|event| event.track_key() == Some(index as u32))
            {
                track_events[kept] = *event;
                kept += 1;
            }
            let plane = &mut planes[index][..length];
            plane.fill(0.0);
            render_test_synth(&track_events[..kept], &mut voices[index], RATE, plane).unwrap();
        }
        let refs: Vec<&[f32]> = planes.iter().map(|plane| &plane[..length]).collect();
        mix_tracks(
            &refs,
            &faders,
            &mut left[..length],
            &mut right[..length],
            &mut peaks,
        )
        .unwrap();
        for frame in 0..length {
            out.push(left[frame]);
            out.push(right[frame]);
        }
    })
}

fn assert_same(reference: &[f32], other: &[f32], block: u32) {
    assert_eq!(
        reference.len(),
        other.len(),
        "length differs at block {block}"
    );
    if let Some(index) = reference
        .iter()
        .zip(other)
        .position(|(left, right)| left.to_bits() != right.to_bits())
    {
        panic!(
            "block {block}: sample {index} is {} but block {} gave {}",
            other[index], BLOCKS[0], reference[index]
        );
    }
}

fn assert_block_independent(render: impl Fn(u32) -> Vec<f32>) {
    let reference = render(BLOCKS[0]);
    assert!(
        reference.iter().any(|sample| *sample != 0.0),
        "the render is silent"
    );
    for block in &BLOCKS[1..] {
        assert_same(&reference, &render(*block), *block);
    }
}

#[test]
fn a_clipped_note_does_not_depend_on_the_block_size() {
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project.insert_note(track, note(0.0, 4.0, 60)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(track, clip, Tick::new(0), quarters(1.0))
        .unwrap();
    assert_block_independent(|block| render_synth(&project, 48_000, block, None));
}

#[test]
fn the_test_synth_does_not_depend_on_the_block_size() {
    let project = project();
    assert_block_independent(|block| render_synth(&project, 36_000, block, None));
}

#[test]
fn a_loop_does_not_depend_on_the_block_size() {
    let project = project();
    let looped = Some((Tick::new(0), quarters(1.0)));
    assert_block_independent(|block| render_synth(&project, 60_000, block, looped));
}

#[test]
fn audio_clip_edges_do_not_depend_on_the_block_size() {
    let samples = vec![0.5f32; 30_000];
    assert_block_independent(|block| {
        drive(36_000, block, None, |spans, length, out| {
            let mut buffer = vec![0.0f32; length];
            let clip = AudioClip {
                start_sample: 1_000,
                samples: &samples,
            };
            render_audio_clips(&[clip], spans, RATE, &mut buffer).unwrap();
            out.extend_from_slice(&buffer);
        })
    });
}

#[test]
fn audit_eng2_the_metronome_does_not_depend_on_the_block_size() {
    assert_block_independent(|block| {
        drive(48_000, block, None, |spans, length, out| {
            let mut buffer = vec![0.0f32; length];
            render_metronome(spans, &tempo(), Meter::four_four(), RATE, &mut buffer).unwrap();
            out.extend_from_slice(&buffer);
        })
    });
}

#[test]
fn audit_eng2_the_loop_seam_does_not_depend_on_the_block_size() {
    let samples = vec![0.5f32; 30_000];
    let looped = Some((Tick::new(0), quarters(1.0)));
    assert_block_independent(|block| {
        drive(60_000, block, looped, |spans, length, out| {
            let mut buffer = vec![0.0f32; length];
            let clip = AudioClip {
                start_sample: 0,
                samples: &samples,
            };
            render_audio_clips(&[clip], spans, RATE, &mut buffer).unwrap();
            fade_loop_seam(spans, seam_fade_frames(RATE), &mut buffer).unwrap();
            out.extend_from_slice(&buffer);
        })
    });
}
