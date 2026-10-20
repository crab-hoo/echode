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
fn two_renders_match_and_notes_without_an_instrument_are_silent() {
    let project = noted();
    let mut once = vec![0.0; 256];
    let mut twice = vec![0.0; 256];
    render_offline(&project, 48_000, 128, &mut once).unwrap();
    render_offline(&project, 48_000, 128, &mut twice).unwrap();
    assert_eq!(once, twice);
    assert_eq!(once[0], 0.0);
    assert!(once.iter().all(|sample| *sample == 0.0));
    let mut streamed = Vec::new();
    render_offline_blocks(&project, 48_000, 128, BLOCK_FRAMES, |block| {
        streamed.extend_from_slice(block);
        Ok::<(), PlaybackError>(())
    })
    .unwrap();
    assert_eq!(once, streamed);
}

#[test]
fn unhosted_export_refuses_to_ignore_a_recorded_instrument() {
    let project = noted();
    let project = project
        .add_clap_device(
            project.tracks()[0].id,
            echode_model::Id::generate(),
            "dev.echode.sine-pad",
        )
        .unwrap();
    let mut samples = [0.0; 128];
    assert!(matches!(
        render_offline(&project, 48_000, 64, &mut samples),
        Err(PlaybackError::Unsupported(_))
    ));
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
fn the_score_length_does_not_assume_an_instrument_tail() {
    let project = noted();
    assert_eq!(render_span_frames(&project, 48_000).unwrap(), 24_000);
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
        right: None,
        fade_in: 8,
        fade_out: 8,
    }];
    let mut offline = Vec::new();
    render_offline_blocks_with_clips(&project, 48_000, 200, 64, &loaded, |block| {
        offline.extend_from_slice(block);
        Ok::<(), PlaybackError>(())
    })
    .unwrap();
    let (mut control, mut playback) = crate::playback::playback_pair(&project, 48_000, 64).unwrap();
    let samples = Arc::clone(&samples);
    let track_raw = project.tracks()[0].id.raw();
    control.set_clip_loader(Arc::new(move |_project, _rate| {
        Ok(vec![crate::playback::LoadedClip {
            track: track_raw,
            start_sample: 0,
            samples: Arc::clone(&samples),
            right: None,
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
