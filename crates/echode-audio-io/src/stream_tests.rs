use super::*;
use echode_engine::{PlaybackOptions, playback};

fn fixture_clips(track: u128, rate: u32) -> Vec<echode_engine::LoadedClip> {
    // This constant source covers these device-free HAL fixtures only.
    vec![echode_engine::LoadedClip {
        track,
        start_sample: 0,
        samples: vec![0.25; rate as usize * 4].into(),
        right: None,
        window: None,
        gain: 1.0,
        fade_in: 0,
        fade_out: 0,
    }]
}

#[test]
fn delayed_control_pause_preserves_stop_and_trace_matches_native_buffers_under_rtsan() {
    let rate = 48_000;
    let frames = 64; // Device geometry of this fixture, not a product limit.
    for layout in 0..3 {
        for delay in [0, 1, 20] {
            let directory = std::env::temp_dir().join(format!(
                "echode-stop-native-{}-{layout}-{delay}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&directory).unwrap();
            let (trace, mut files) = crate::trace::pair(rate, frames, directory.clone());
            let (project, track) = Project::new().add_track("stop trace");
            let (mut control, player) =
                playback(&project, rate, frames, PlaybackOptions::default()).unwrap();
            let wave = |sample: usize, channel: usize| {
                let (amplitude, frequency) = [(0.25, 437.0), (-0.5, 1601.0)][channel];
                amplitude
                    * (std::f32::consts::TAU * frequency * sample as f32 / rate as f32 + 0.7).sin()
            };
            let left: Arc<[f32]> = (0..rate as usize).map(|i| wave(i, 0)).collect();
            let right: Arc<[f32]> = (0..rate as usize).map(|i| wave(i, 1)).collect();
            control.set_clip_loader(Arc::new(move |_, _| {
                Ok(vec![echode_engine::LoadedClip {
                    track: track.raw(),
                    start_sample: 0,
                    samples: Arc::clone(&left),
                    right: Some(Arc::clone(&right)),
                    window: None,
                    gain: 1.0,
                    fade_in: 0,
                    fade_out: 0,
                }])
            }));
            control.replace(&project).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let cancellation = TransportCancellation::default();
            let callback = Callback {
                render: UnsafeCell::new(Render {
                    player,
                    declick: echode_engine::OutputDeclick::new(rate),
                    output_revision: 0,
                    enabled: true,
                    left: vec![0.0; frames],
                    right: vec![0.0; frames],
                }),
                failed: AtomicBool::new(false),
                panic: CallbackPanic::new("output callback"),
                callbacks: AtomicU64::new(0),
                trace: UnsafeCell::new(Some(trace)),
                channels: if layout == 2 { 1 } else { 2 },
                cycle: crate::io_cycle::IoCycle::new(rate, 0),
                silent: false,
                hardware_epoch: Arc::new(AtomicU64::new(0)),
                observed_epoch: AtomicU64::new(0),
                cancellation: cancellation.clone(),
                transport_revision: AtomicU64::new(0),
            };
            let mut interleaved = vec![0.0f32; frames * if layout == 2 { 1 } else { 2 }];
            let mut planar = [vec![0.0f32; frames], vec![0.0f32; frames]];
            let mut history = Vec::<[f32; 2]>::new();
            let mut run = || {
                let mut buffers: Vec<_> = if layout == 1 {
                    planar
                        .iter_mut()
                        .map(|plane| AudioBuffer {
                            mNumberChannels: 1,
                            mDataByteSize: std::mem::size_of_val(plane.as_slice()) as u32,
                            mData: plane.as_mut_ptr().cast(),
                        })
                        .collect()
                } else {
                    vec![AudioBuffer {
                        mNumberChannels: if layout == 2 { 1 } else { 2 },
                        mDataByteSize: std::mem::size_of_val(interleaved.as_slice()) as u32,
                        mData: interleaved.as_mut_ptr().cast(),
                    }]
                };
                unsafe {
                    fill_with_time(
                        &callback,
                        &mut buffers,
                        Entry {
                            at: Instant::now(),
                            host: 0,
                            output_host: Some(history.len() as u64 + 1),
                        },
                    )
                };
                for frame in 0..frames {
                    history.push(match layout {
                        1 => [planar[0][frame], planar[1][frame]],
                        2 => [interleaved[frame]; 2],
                        _ => [interleaved[frame * 2], interleaved[frame * 2 + 1]],
                    });
                }
            };
            for _ in 0..20 {
                run();
                files.service(control.status());
            }
            let stopped = control.status().sample;
            cancellation.request(&crate::AudioCommand::Stop);
            for index in 0..60 {
                if index == delay {
                    control.command(PlaybackCommand::Pause).unwrap();
                }
                run();
                files.service(control.status());
                assert_eq!(control.status().sample, stopped);
            }
            let fade = (rate / 200) as usize;
            for (age, output) in history[stopped as usize..].iter().enumerate() {
                let gain = if age >= fade - 1 {
                    0.0
                } else {
                    0.5 * (1.0 + (std::f64::consts::PI * age as f64 / (fade - 1) as f64).cos())
                        as f32
                };
                let mut expected = [
                    wave(stopped as usize + age, 0) * gain,
                    wave(stopped as usize + age, 1) * gain,
                ];
                if layout == 2 {
                    expected = [(expected[0] + expected[1]) * std::f32::consts::FRAC_1_SQRT_2; 2];
                }
                for channel in 0..2 {
                    assert!(
                        (output[channel] - expected[channel]).abs() < 1.0e-6,
                        "late Pause changed output at {layout}/{delay}/{age}: {} != {}",
                        output[channel],
                        expected[channel]
                    );
                }
            }
            let captures: Vec<_> = std::fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "json")
                })
                .collect();
            assert_eq!(captures.len(), 1);
            let metadata: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&captures[0]).unwrap()).unwrap();
            assert_eq!(metadata["stop_frame"], stopped as u64);
            assert_eq!(metadata["stop_retained"], true);
            let first = metadata["first_frame"].as_u64().unwrap() as usize;
            let pcm = std::fs::read(directory.join(metadata["pcm"].as_str().unwrap())).unwrap();
            for (index, bytes) in pcm.chunks_exact(8).enumerate() {
                for channel in 0..2 {
                    let value =
                        u32::from_le_bytes(bytes[channel * 4..channel * 4 + 4].try_into().unwrap());
                    assert_eq!(value, history[first + index][channel].to_bits());
                }
            }
            assert!(!control.status().failed);
            assert!(!callback.failed.load(Ordering::Acquire));
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn contract_transport_discontinuities_declick_hal_output_under_rtsan() {
    // Clip and scratch lengths cover this transport fixture only; they do
    // not constrain product tracks, channels, or device buffer sizes.
    for rate in [48_000, 96_000] {
        for frames in [1, 7, 64, 512] {
            for command in [
                Some(PlaybackCommand::Stop),
                Some(PlaybackCommand::Pause),
                None,
            ] {
                let cancellation_only = command.is_none();
                let (project, track) = Project::new().add_track("continuous audio");
                let (mut control, player) =
                    playback(&project, rate, frames, PlaybackOptions::default()).unwrap();
                control.set_clip_loader(Arc::new(move |_, _| {
                    Ok(vec![echode_engine::LoadedClip {
                        track: track.raw(),
                        start_sample: 0,
                        samples: vec![0.25; 10_000].into(),
                        right: Some(vec![-0.5; 10_000].into()),
                        window: None,
                        gain: 1.0,
                        fade_in: 0,
                        fade_out: 0,
                    }])
                }));
                control.replace(&project).unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                let cancellation = TransportCancellation::default();
                let callback = Callback {
                    render: UnsafeCell::new(Render {
                        player,
                        declick: echode_engine::OutputDeclick::new(rate),
                        output_revision: 0,
                        enabled: true,
                        left: vec![0.0; frames],
                        right: vec![0.0; frames],
                    }),
                    failed: AtomicBool::new(false),
                    panic: CallbackPanic::new("output callback"),
                    callbacks: AtomicU64::new(0),
                    trace: UnsafeCell::new(None),
                    channels: 2,
                    cycle: crate::io_cycle::IoCycle::new(rate, 0),
                    silent: false,
                    hardware_epoch: Arc::new(AtomicU64::new(0)),
                    observed_epoch: AtomicU64::new(0),
                    cancellation: cancellation.clone(),
                    transport_revision: AtomicU64::new(0),
                };
                let mut stereo = vec![0.0f32; frames * 2];
                let mut run = || {
                    let buffer = AudioBuffer {
                        mNumberChannels: 2,
                        mDataByteSize: std::mem::size_of_val(stereo.as_slice()) as u32,
                        mData: stereo.as_mut_ptr().cast(),
                    };
                    unsafe { fill(&callback, &mut [buffer], Instant::now()) };
                    stereo.clone()
                };
                let fade = (rate / 200) as usize;
                for _ in 0..fade.div_ceil(frames) + 1 {
                    run();
                }
                let position = control.status().sample;
                if let Some(command) = command {
                    control.command(command).unwrap();
                } else {
                    cancellation.request(&crate::AudioCommand::Stop);
                }
                let mut output = Vec::new();
                for _ in 0..fade.div_ceil(frames) + 2 {
                    output.extend(run());
                }
                assert_eq!(control.status().sample, position);
                assert!(!control.status().playing);
                // The stopped clock must not cut a nonzero waveform to zero.
                assert_eq!(
                    &output[..2],
                    &[0.25, -0.5],
                    "stop edge {rate}/{frames}/{cancellation_only}"
                );
                for (index, pair) in output.chunks_exact(2).enumerate() {
                    let gain = if index >= fade - 1 {
                        0.0
                    } else {
                        0.5 * (1.0
                            + (std::f64::consts::PI * index as f64 / (fade - 1) as f64).cos())
                            as f32
                    };
                    for channel in 0..2 {
                        assert!((pair[channel] - [0.25, -0.5][channel] * gain).abs() < 1.0e-7);
                        if index >= fade - 1 {
                            assert_eq!(pair[channel], 0.0);
                        }
                    }
                }
                assert!(!callback.failed.load(Ordering::Acquire));
            }
        }
    }
}

#[test]
fn contract_stop_preserves_waveform_while_fading_under_rtsan() {
    // Rates, phases and buffers bound only this regression fixture.
    for rate in [48_000, 96_000] {
        for frames in [1, 7, 64, 512] {
            for phase in [0.0f32, 0.7, 1.6] {
                for command in [
                    Some(PlaybackCommand::Stop),
                    Some(PlaybackCommand::Pause),
                    None,
                ] {
                    let sample = move |index: usize, channel: usize| {
                        let (amplitude, frequency) = [(0.25, 437.0), (0.5, 1601.0)][channel];
                        amplitude
                            * (std::f32::consts::TAU * frequency * index as f32 / rate as f32
                                + phase)
                                .sin()
                    };
                    let (project, track) = Project::new().add_track("waveform");
                    let (mut control, player) =
                        playback(&project, rate, frames, PlaybackOptions::default()).unwrap();
                    let length = rate as usize;
                    let pcm_left: Arc<[f32]> = (0..length).map(|i| sample(i, 0)).collect();
                    let pcm_right: Arc<[f32]> = (0..length).map(|i| sample(i, 1)).collect();
                    control.set_clip_loader(Arc::new(move |_, _| {
                        Ok(vec![echode_engine::LoadedClip {
                            track: track.raw(),
                            start_sample: 0,
                            samples: Arc::clone(&pcm_left),
                            right: Some(Arc::clone(&pcm_right)),
                            window: None,
                            gain: 1.0,
                            fade_in: 0,
                            fade_out: 0,
                        }])
                    }));
                    control.replace(&project).unwrap();
                    control.command(PlaybackCommand::Play).unwrap();
                    let cancellation = TransportCancellation::default();
                    let callback = Callback {
                        render: UnsafeCell::new(Render {
                            player,
                            declick: echode_engine::OutputDeclick::new(rate),
                            output_revision: 0,
                            enabled: true,
                            left: vec![0.0; frames],
                            right: vec![0.0; frames],
                        }),
                        failed: AtomicBool::new(false),
                        panic: CallbackPanic::new("output callback"),
                        callbacks: AtomicU64::new(0),
                        trace: UnsafeCell::new(None),
                        channels: 2,
                        cycle: crate::io_cycle::IoCycle::new(rate, 0),
                        silent: false,
                        hardware_epoch: Arc::new(AtomicU64::new(0)),
                        observed_epoch: AtomicU64::new(0),
                        cancellation: cancellation.clone(),
                        transport_revision: AtomicU64::new(0),
                    };
                    let mut output = vec![0.0f32; frames * 2];
                    let mut run = || {
                        let buffer = AudioBuffer {
                            mNumberChannels: 2,
                            mDataByteSize: std::mem::size_of_val(output.as_slice()) as u32,
                            mData: output.as_mut_ptr().cast(),
                        };
                        unsafe {
                            fill(&callback, &mut [buffer], Instant::now());
                        }
                        output.clone()
                    };
                    let fade = (rate / 200) as usize;
                    for _ in 0..(fade * 3).div_ceil(frames) {
                        run();
                    }
                    let stopped = control.status().sample as usize;
                    if let Some(command) = command {
                        control.command(command).unwrap();
                    } else {
                        cancellation.request(&crate::AudioCommand::Stop);
                    }
                    let mut heard = Vec::new();
                    for _ in 0..fade.div_ceil(frames) + 2 {
                        heard.extend(run());
                    }
                    for (offset, pair) in heard.chunks_exact(2).enumerate() {
                        let gain = if offset >= fade - 1 {
                            0.0
                        } else {
                            0.5 * (1.0
                                + (std::f64::consts::PI * offset as f64 / (fade - 1) as f64).cos())
                                as f32
                        };
                        for (channel, value) in pair.iter().enumerate() {
                            let expected = sample(stopped + offset, channel) * gain;
                            assert!(
                                (*value - expected).abs() < 2.0e-6,
                                "stop lost waveform at {rate}/{frames}, phase {phase}, offset {offset}, channel {channel}: {} != {expected}",
                                pair[channel]
                            );
                        }
                    }
                    assert_eq!(control.status().sample as usize, stopped);
                    assert!(!control.status().playing);
                    assert!(!callback.failed.load(Ordering::Acquire));
                }
            }
        }
    }
}

#[test]
fn in_flight_stop_preserves_heard_audio_and_cancels_unheard_starts_under_rtsan() {
    struct Clock {
        sample: i64,
        armed: Arc<AtomicBool>,
        cancellation: TransportCancellation,
    }
    impl echode_engine::ClockSource for Clock {
        fn position(&self) -> i64 {
            self.sample
        }
        fn locate(&mut self, sample: i64) {
            self.sample = sample;
            if self.armed.swap(false, Ordering::AcqRel) {
                self.cancellation.request(&crate::AudioCommand::Stop);
            }
        }
        fn advance(&mut self, frames: u32) -> Result<(), echode_engine::PlaybackError> {
            self.locate(self.sample + i64::from(frames));
            Ok(())
        }
    }
    for already_heard in [false, true] {
        let rate = 48_000;
        let frames = 64; // Fixture buffer size only.
        let cancellation = TransportCancellation::default();
        let armed = Arc::new(AtomicBool::new(false));
        let (project, track) = Project::new().add_track("continuing PCM");
        let (mut control, player) = echode_engine::playback(
            &project,
            rate,
            frames,
            echode_engine::PlaybackOptions::default().clock(Box::new(Clock {
                sample: 0,
                armed: Arc::clone(&armed),
                cancellation: cancellation.clone(),
            })),
        )
        .unwrap();
        control.set_clip_loader(Arc::new(move |_, _| {
            Ok(vec![echode_engine::LoadedClip {
                track: track.raw(),
                start_sample: 0,
                samples: vec![0.25; 4096].into(),
                right: Some(vec![-0.5; 4096].into()),
                window: None,
                gain: 1.0,
                fade_in: 0,
                fade_out: 0,
            }])
        }));
        control.replace(&project).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let callback = Callback {
            render: UnsafeCell::new(Render {
                player,
                declick: echode_engine::OutputDeclick::new(rate),
                output_revision: 0,
                enabled: true,
                left: vec![0.0; frames],
                right: vec![0.0; frames],
            }),
            failed: AtomicBool::new(false),
            panic: CallbackPanic::new("output callback"),
            callbacks: AtomicU64::new(0),
            trace: UnsafeCell::new(None),
            channels: 2,
            cycle: crate::io_cycle::IoCycle::new(rate, 0),
            silent: false,
            hardware_epoch: Arc::new(AtomicU64::new(0)),
            observed_epoch: AtomicU64::new(0),
            cancellation,
            transport_revision: AtomicU64::new(0),
        };
        let mut output = vec![0.0f32; frames * 2];
        let mut run = || {
            let buffer = AudioBuffer {
                mNumberChannels: 2,
                mDataByteSize: std::mem::size_of_val(output.as_slice()) as u32,
                mData: output.as_mut_ptr().cast(),
            };
            unsafe {
                fill(&callback, &mut [buffer], Instant::now());
            }
            output.clone()
        };
        if already_heard {
            for _ in 0..8 {
                run();
            }
        }
        let before = control.status().sample;
        armed.store(true, Ordering::Release);
        let heard = run();
        if already_heard {
            assert!(
                heard.chunks_exact(2).all(|pair| pair == [0.25, -0.5]),
                "in-flight output: {:?}, status: {:?}",
                &heard[..8],
                control.status()
            );
            assert_eq!(control.status().sample, before + frames as i64);
        } else {
            assert!(heard.iter().all(|sample| *sample == 0.0));
            assert_eq!(control.status().sample, before);
            let meters = control.meters();
            assert!(!meters.is_empty(), "cancelled start had no meter");
            let peak = meters
                .iter()
                .map(|meter| meter.peak())
                .fold(0.0f32, f32::max);
            assert_eq!(peak, 0.0, "cancelled start kept a meter peak");
        }
        let stopped = control.status().sample;
        for _ in 0..8 {
            run();
            assert_eq!(control.status().sample, stopped);
        }
        assert!(!control.status().playing);
        assert!(run().iter().all(|sample| *sample == 0.0));
        assert!(!callback.failed.load(Ordering::Acquire));
    }
}

#[test]
fn retained_stream_publishes_a_stopped_locate_before_its_callback() {
    let project = Project::new();
    let (control, mut player) =
        prepare_playback(&project, 48000, 64, 0, &TransportSetup::idle()).unwrap();
    let mut stream = OutputStream {
        workgroup_listener: None,
        device: OutputDevice {
            id: 0,
            uid: "fixture".into(),
            name: "fixture".into(),
            sample_rate: 48000,
            buffer_frames: 64,
            safety_frames: 0,
            channels: 2,
            formats: Vec::new(),
        },
        control,
        callback: None,
        proc: None,
        registered: false,
        package: None,
        pending_locate: None,
        trace_files: None,
    };
    stream.pause().unwrap();
    stream.locate(&project, 705_600 * 2).unwrap();
    assert_eq!(stream.status().tick, 705_600 * 2);
    // Another command may be consumed in the same callback as Locate.
    stream.resume(0).unwrap();
    let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
    player.render_gated_until(
        &mut left,
        &mut right,
        Instant::now() + Duration::from_secs(1),
        false,
    );
    assert_eq!(stream.status().tick, 705_600 * 2);
    stream.resume(0).unwrap();
    player.render(&mut left, &mut right);
    assert_eq!(stream.status().sample, 48000 + 64);
}

#[test]
fn contract_stop_during_preparation_gates_callbacks_under_rtsan() {
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            echode_model::NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(4).unwrap(),
            },
        )
        .unwrap()
        .0;
    let cancellation = TransportCancellation::default();
    let mut setup = TransportSetup::idle();
    setup.cancellation = cancellation.clone();
    setup.revision = cancellation.request(&crate::AudioCommand::Play);
    let interrupt = cancellation.clone();
    // The product media loader is called during preparation, before HAL
    // can start. Submit Stop from that phase, without relying on timing.
    setup.clips = Arc::new(move |_, _| {
        interrupt.request(&crate::AudioCommand::Stop);
        Ok(fixture_clips(track.raw(), 48_000))
    });
    let (mut control, player) = prepare_playback(&project, 48000, 64, 0, &setup).unwrap();
    let callback = Callback {
        render: UnsafeCell::new(Render {
            player,
            declick: echode_engine::OutputDeclick::new(48000),
            output_revision: 0,
            enabled: true,
            left: vec![0.0; 64],
            right: vec![0.0; 64],
        }),
        failed: AtomicBool::new(false),
        panic: CallbackPanic::new("output callback"),
        callbacks: AtomicU64::new(0),
        trace: UnsafeCell::new(None),
        channels: 2,
        cycle: crate::io_cycle::IoCycle::new(48000, 0),
        silent: false,
        hardware_epoch: Arc::new(AtomicU64::new(0)),
        observed_epoch: AtomicU64::new(0),
        cancellation: cancellation.clone(),
        transport_revision: AtomicU64::new(setup.revision),
    };
    // Fixed storage covers only this 64-frame stereo HAL fixture.
    let mut stereo = [1.0f32; 128];
    let mut run = || {
        let buffer = AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: std::mem::size_of_val(&stereo) as u32,
            mData: stereo.as_mut_ptr().cast(),
        };
        unsafe { fill(&callback, &mut [buffer], Instant::now()) };
        stereo
    };
    assert!(run().iter().all(|sample| *sample == 0.0));
    assert_eq!(control.status().sample, 0);
    assert!(!control.status().playing);
    // A fresh Play reuses the prepared player. Stop gates it immediately,
    // even if the control worker has not consumed the queued stop yet.
    let revision = cancellation.request(&crate::AudioCommand::Play);
    control.command(PlaybackCommand::Play).unwrap();
    callback
        .transport_revision
        .store(revision, Ordering::Release);
    assert!(run().iter().any(|sample| *sample != 0.0));
    let held = control.status().sample;
    cancellation.request(&crate::AudioCommand::Stop);
    for block in 0..5 {
        let output = run();
        if block == 4 {
            assert!(output.iter().all(|sample| *sample == 0.0));
        }
        assert_eq!(control.status().sample, held);
    }
    assert!(!callback.failed.load(Ordering::Acquire));
}
use std::time::{Duration, Instant};

#[test]
fn preview_before_open_applies_before_any_live_output_under_rtsan() {
    let (project, track) = Project::new().add_track("preview before play");
    let project = project
        .insert_note(
            track,
            echode_model::NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(4).unwrap(),
            },
        )
        .unwrap()
        .0;
    for rate in [48_000, 96_000] {
        let mut setup = TransportSetup::idle();
        setup.clips = Arc::new(move |_, _| Ok(fixture_clips(track.raw(), rate)));
        setup.fader_preview = Some(vec![(track.raw(), 0.0, 0.0)]);
        let (mut control, mut player) = prepare_playback(&project, rate, 64, 0, &setup).unwrap();
        // These buffers cover only this product preparation fixture.
        let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
        for _ in 0..16 {
            player.render_live_gated_until(
                &mut left,
                &mut right,
                Instant::now() + Duration::from_secs(5),
                true,
            );
            assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
        }
        assert_eq!(control.status().sample, 1024);
        control.clear_fader_preview(false).unwrap();
        for _ in 0..16 {
            player.render_live_gated_until(
                &mut left,
                &mut right,
                Instant::now() + Duration::from_secs(5),
                true,
            );
        }
        assert!(left.iter().chain(&right).any(|sample| *sample != 0.0));
        assert!(!control.status().failed);
    }
}

#[test]
fn contract_startup_position_is_published_before_the_first_callback() {
    let project = Project::new();
    // Rates and scratch lengths are this regression's cases, not product limits.
    for rate in [48_000, 96_000] {
        for frames in [1, 64, 512] {
            for beat in [2, 7] {
                let tick = beat * echode_model::TICKS_PER_QUARTER;
                let sample = tick_to_sample(Tick::new(tick), project.tempo(), rate).unwrap();
                let (control, mut player) =
                    prepare_playback(&project, rate, frames, tick, &TransportSetup::idle())
                        .unwrap();
                assert_eq!(
                    control.status().tick,
                    tick,
                    "a newly opened stream published zero before its first callback"
                );
                assert_eq!(control.status().sample, sample);
                let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
                player.render(&mut left, &mut right);
                assert_eq!(control.status().sample, sample + frames as i64);
                assert!(!control.status().failed);
            }
        }
    }
}

#[test]
fn interleaved_and_planar_hal_buffers_match_under_rtsan() {
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            echode_model::NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0;
    let callback = || {
        let (mut control, player) =
            playback(&project, 48000, 64, PlaybackOptions::default()).unwrap();
        control.set_clip_loader(Arc::new(move |_, _| Ok(fixture_clips(track.raw(), 48_000))));
        control.replace(&project).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        (
            control,
            Callback {
                render: UnsafeCell::new(Render {
                    player,
                    declick: echode_engine::OutputDeclick::new(48000),
                    output_revision: 0,
                    enabled: true,
                    left: vec![0.0; 64],
                    right: vec![0.0; 64],
                }),
                failed: AtomicBool::new(false),
                panic: CallbackPanic::new("output callback"),
                callbacks: AtomicU64::new(0),
                trace: UnsafeCell::new(None),
                channels: 2,
                cycle: crate::io_cycle::IoCycle::new(48_000, 0),
                silent: false,
                hardware_epoch: Arc::new(AtomicU64::new(0)),
                observed_epoch: AtomicU64::new(0),
                cancellation: TransportCancellation::default(),
                transport_revision: AtomicU64::new(0),
            },
        )
    };
    let (control_a, a) = callback();
    let (control_b, b) = callback();
    let (mut stereo, mut left, mut right, mut extra) =
        ([0.0f32; 128], [0.0f32; 64], [0.0f32; 64], [1.0f32; 64]);
    let buffer = |data: &mut [f32], channels| AudioBuffer {
        mNumberChannels: channels,
        mDataByteSize: std::mem::size_of_val(data) as u32,
        mData: data.as_mut_ptr().cast(),
    };
    unsafe {
        fill(&a, &mut [buffer(&mut stereo, 2)], Instant::now());
    }
    unsafe {
        fill(
            &b,
            &mut [
                buffer(&mut left, 1),
                buffer(&mut right, 1),
                buffer(&mut extra, 1),
            ],
            Instant::now(),
        );
    }
    for (frame, (left, right)) in stereo.chunks_exact(2).zip(left.iter().zip(&right)) {
        assert_eq!(frame, &[*left, *right]);
    }
    assert!(extra.iter().all(|sample| *sample == 0.0));
    assert!(stereo.iter().any(|sample| *sample != 0.0));
    assert_eq!(control_a.status().sample, control_b.status().sample);
    let held = control_a.status().sample;
    a.hardware_epoch.fetch_add(1, Ordering::Release);
    stereo.fill(1.0);
    unsafe { fill(&a, &mut [buffer(&mut stereo, 2)], Instant::now()) };
    assert!(stereo.iter().all(|sample| *sample == 0.0));
    assert_eq!(control_a.status().sample, held);
    a.observed_epoch.store(1, Ordering::Release);
    unsafe { fill(&a, &mut [buffer(&mut stereo, 2)], Instant::now()) };
    assert_eq!(control_a.status().sample, held + 64);
    assert!(stereo.iter().any(|sample| *sample != 0.0));
    assert!(!a.failed.load(Ordering::Acquire) && !b.failed.load(Ordering::Acquire));
    // A driver enlargement beyond prepared scratch is silent, never a panic.
    let mut large = [1.0f32; 256];
    unsafe {
        fill(&a, &mut [buffer(&mut large, 2)], Instant::now());
    }
    assert!(large.iter().all(|sample| *sample == 0.0));
    assert!(a.failed.load(Ordering::Acquire));
}

#[test]
fn the_hal_deadline_starts_when_the_callback_is_entered() {
    let entered = Instant::now();
    std::thread::sleep(Duration::from_millis(30));
    let spent = entered + echode_engine::block_duration(64, 48_000);
    let restarted = Instant::now() + echode_engine::block_duration(64, 48_000);
    assert!(spent < Instant::now());
    assert!(restarted > spent + Duration::from_millis(20));
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            echode_model::NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0;
    let (mut control, player) = playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| Ok(fixture_clips(track.raw(), 48_000))));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let callback = Callback {
        render: UnsafeCell::new(Render {
            player,
            declick: echode_engine::OutputDeclick::new(48000),
            output_revision: 0,
            enabled: true,
            left: vec![0.0; 64],
            right: vec![0.0; 64],
        }),
        failed: AtomicBool::new(false),
        panic: CallbackPanic::new("output callback"),
        callbacks: AtomicU64::new(0),
        trace: UnsafeCell::new(None),
        channels: 2,
        cycle: crate::io_cycle::IoCycle::new(48_000, 0),
        silent: false,
        hardware_epoch: Arc::new(AtomicU64::new(0)),
        observed_epoch: AtomicU64::new(0),
        cancellation: TransportCancellation::default(),
        transport_revision: AtomicU64::new(0),
    };
    let mut stereo = [0.0f32; 128];
    let buffer = AudioBuffer {
        mNumberChannels: 2,
        mDataByteSize: std::mem::size_of_val(&stereo) as u32,
        mData: stereo.as_mut_ptr().cast(),
    };
    unsafe { fill(&callback, &mut [buffer], entered) };
    assert!(stereo.iter().any(|sample| *sample != 0.0));
    assert!(!callback.failed.load(Ordering::Acquire));
}

/// A playing callback whose device keeps `safety_frames` ahead, where one
/// host tick is one nanosecond.
fn timed_callback(safety_frames: u32) -> (Callback, PlaybackControl) {
    let (project, track) = Project::new().add_track("pad");
    let (mut control, player) = playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| Ok(fixture_clips(track.raw(), 48_000))));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let callback = Callback {
        render: UnsafeCell::new(Render {
            player,
            declick: echode_engine::OutputDeclick::new(48000),
            output_revision: 0,
            enabled: true,
            left: vec![0.0; 64],
            right: vec![0.0; 64],
        }),
        failed: AtomicBool::new(false),
        panic: CallbackPanic::new("output callback"),
        callbacks: AtomicU64::new(0),
        trace: UnsafeCell::new(None),
        channels: 2,
        cycle: crate::io_cycle::IoCycle::with_timebase(48_000, safety_frames, Some((1, 1))),
        silent: false,
        hardware_epoch: Arc::new(AtomicU64::new(0)),
        observed_epoch: AtomicU64::new(0),
        cancellation: TransportCancellation::default(),
        transport_revision: AtomicU64::new(0),
    };
    (callback, control)
}

#[test]
fn contract_the_output_block_ends_before_the_device_safety_offset() {
    // 48 frames of safety are 1 ms at 48 kHz.
    let (callback, _control) = timed_callback(48);
    let block = echode_engine::block_duration(64, 48_000);
    let host = 5_000_000_000;
    let render = |output_host| {
        let mut stereo = [0.0f32; 128];
        let at = Instant::now();
        let entry = Entry {
            at,
            host,
            output_host,
        };
        let buffer = AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: std::mem::size_of_val(&stereo) as u32,
            mData: stereo.as_mut_ptr().cast(),
        };
        let deadline = unsafe { fill_inner(&callback, &mut [buffer], entry) };
        assert!(stereo.iter().any(|sample| *sample != 0.0));
        deadline.map(|deadline| deadline.saturating_duration_since(at))
    };
    // HAL entered late, 2 ms before the output time: the device needs the
    // block 1 ms before it, and the audition and HAL keep 1/16 of the block.
    assert_eq!(
        render(Some(host + 2_000_000)),
        Some(Duration::from_millis(1) - block / 16)
    );
    // Already inside the safety offset: polling once is all that can help.
    assert_eq!(render(Some(host + 500_000)), Some(Duration::ZERO));
    // An output time HAL marked invalid keeps one block from entry.
    assert_eq!(render(None), Some(block));
    assert!(!callback.failed.load(Ordering::Acquire));
}

#[test]
#[ignore = "needs a CoreAudio output; make spike-audio runs the silent probe"]
fn a_live_output_runs_silently_and_stops_repeatedly() {
    let inventory = crate::inventory().expect("output inventory");
    let device = inventory
        .devices
        .iter()
        .find(|device| device.id == inventory.default_device)
        .expect("default output");
    for _ in 0..4 {
        let mut stream =
            OutputStream::open_mode(device.clone(), &Project::new(), 0, true).expect("open output");
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            stream.service().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(stream.callbacks() > 0);
        let status = stream.close();
        assert!(status.sample > 0 && !status.failed);
    }
}

#[test]
fn a_stream_opened_for_devices_stays_stopped_until_play() {
    let (project, track) = Project::new().add_track("audio");
    let mut setup = TransportSetup::idle();
    setup.start = false;
    setup.clips = Arc::new(move |_, rate| Ok(fixture_clips(track.raw(), rate)));
    let (mut control, mut player) = prepare_playback(&project, 48000, 64, 0, &setup).unwrap();
    let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
    // No Stop was ever requested, so only the missing Play keeps this silent.
    for _ in 0..4 {
        player.render(&mut left, &mut right);
        control.service();
    }
    assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
    assert!(!control.status().playing);
    assert_eq!(control.status().sample, 0);
    control.command(PlaybackCommand::Play).unwrap();
    player.render(&mut left, &mut right);
    assert!(control.status().playing);
    assert!(left.iter().any(|sample| *sample != 0.0));
}

/// DOC-A2 (ADR 0229): a device whose helper holds an unrecorded change gets
/// a route with that helper's bytes, so the replacement keeps the helper.
/// Other devices still restore their recorded objects.
#[test]
fn a_live_device_route_carries_its_helper_bytes_instead_of_the_recorded_object() {
    use echode_model::Id;
    let package = std::env::temp_dir().join(format!("echode-live-routes-{}", Id::generate()));
    let recorded = echode_history::write_object(&package, b"recorded").unwrap();
    let (project, track) = Project::new().add_track("fx");
    let (held, fresh, other) = (Id::from_raw(0x1A), Id::from_raw(0x1B), Id::from_raw(0x1C));
    let mut project = project;
    for device in [held, fresh, other] {
        project = project
            .add_clap_device(track, device, "dev.echode.test-params")
            .unwrap()
            .set_device_state(device, Some(recorded.clone()))
            .unwrap();
    }
    let installed = [PluginRoute {
        clap_id: "dev.echode.test-params".into(),
        path: "/nowhere/params.clap".into(),
        instrument: false,
        fault: None,
        state: None,
    }];
    let routes = routes_with_states(&project, &package, &installed, |device| {
        if device == held.raw() {
            Some(Some(b"held".to_vec()))
        } else if device == fresh.raw() {
            Some(None)
        } else {
            None
        }
    })
    .unwrap();
    let states: Vec<_> = routes.iter().map(|route| route.state.as_deref()).collect();
    assert_eq!(
        states,
        [Some(&b"held"[..]), None, Some(&b"recorded"[..])],
        "one route per device, in project order"
    );
    let _ = std::fs::remove_dir_all(package);
}

/// XCA-14 (ADR 0238): a panic inside the product render stops at the
/// callback instead of unwinding into HAL. That block and every later one
/// are silent, the player is not entered again, and the control thread
/// reports where it panicked.
#[test]
fn a_panic_in_the_render_leaves_the_output_silent_and_reports_its_location() {
    let log = crate::panic_log::tests::test_log();
    // A live block moves the clock with `locate` after it has rendered.
    struct Clock {
        sample: i64,
        moves: Arc<AtomicU64>,
        panic_at: i64,
    }
    impl echode_engine::ClockSource for Clock {
        fn position(&self) -> i64 {
            self.sample
        }
        fn locate(&mut self, sample: i64) {
            self.moves.fetch_add(1, Ordering::Relaxed);
            if sample >= self.panic_at {
                panic!("render marker at sample {sample}");
            }
            self.sample = sample;
        }
        fn advance(&mut self, frames: u32) -> Result<(), echode_engine::PlaybackError> {
            self.locate(self.sample + i64::from(frames));
            Ok(())
        }
    }
    let rate = 48_000;
    let frames = 64; // Fixture buffer size only.
    let moves = Arc::new(AtomicU64::new(0));
    let (project, track) = Project::new().add_track("pcm");
    let (mut control, player) = playback(
        &project,
        rate,
        frames,
        PlaybackOptions::default().clock(Box::new(Clock {
            sample: 0,
            moves: Arc::clone(&moves),
            panic_at: frames as i64 * 3,
        })),
    )
    .unwrap();
    control.set_clip_loader(Arc::new(move |_, rate| {
        Ok(fixture_clips(track.raw(), rate))
    }));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let callback = Callback {
        render: UnsafeCell::new(Render {
            player,
            declick: echode_engine::OutputDeclick::new(rate),
            output_revision: 0,
            enabled: true,
            left: vec![0.0; frames],
            right: vec![0.0; frames],
        }),
        failed: AtomicBool::new(false),
        panic: CallbackPanic::new("output callback"),
        callbacks: AtomicU64::new(0),
        trace: UnsafeCell::new(None),
        channels: 2,
        cycle: crate::io_cycle::IoCycle::new(rate, 0),
        silent: false,
        hardware_epoch: Arc::new(AtomicU64::new(0)),
        observed_epoch: AtomicU64::new(0),
        cancellation: TransportCancellation::default(),
        transport_revision: AtomicU64::new(0),
    };
    let mut stream = OutputStream {
        workgroup_listener: None,
        device: OutputDevice {
            id: 0,
            uid: "fixture".into(),
            name: "fixture".into(),
            sample_rate: rate,
            buffer_frames: frames as u32,
            safety_frames: 0,
            channels: 2,
            formats: Vec::new(),
        },
        control,
        callback: Some(Box::new(callback)),
        proc: None,
        registered: false,
        package: None,
        pending_locate: None,
        trace_files: None,
    };
    let mut output = vec![0.0f32; frames * 2];
    let mut run = |stream: &OutputStream| {
        output.fill(1.0);
        let buffer = AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: std::mem::size_of_val(output.as_slice()) as u32,
            mData: output.as_mut_ptr().cast(),
        };
        let callback = stream.callback.as_deref().unwrap();
        unsafe { fill(callback, &mut [buffer], Instant::now()) };
        output.clone()
    };
    for _ in 0..2 {
        assert!(run(&stream).iter().any(|sample| *sample != 0.0));
        stream.service().unwrap();
    }
    // The third block renders, then panics while moving the clock to 192.
    assert!(run(&stream).iter().all(|sample| *sample == 0.0));
    let entered = moves.load(Ordering::Relaxed);
    for _ in 0..4 {
        assert!(run(&stream).iter().all(|sample| *sample == 0.0));
    }
    assert_eq!(
        moves.load(Ordering::Relaxed),
        entered,
        "the player ran again"
    );
    let callback = stream.callback.as_deref().unwrap();
    assert!(callback.failed.load(Ordering::Acquire));
    let error = stream.service().unwrap_err();
    assert!(
        error.starts_with("audio playback stopped because the audio callback panicked: ")
            && error.contains("stream_tests.rs:")
            && error.ends_with("render marker at sample 192"),
        "{error}"
    );
    assert_eq!(stream.service().unwrap_err(), error);
    let lines: Vec<_> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .filter(|line| line.contains("render marker at sample 192"))
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["source"], "output callback");
    stream.close();
}
