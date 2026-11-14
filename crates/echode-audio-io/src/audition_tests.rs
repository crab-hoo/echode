use super::*;
#[path = "audition_guard_tests.rs"]
mod guard_tests;
use std::sync::atomic::AtomicUsize;

// Rates, lengths and buffer sizes below shape these device-free fixtures only.
const RATE: u32 = 48_000;
const FADE: usize = (RATE / 200) as usize;

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> Fixture {
    let directory = std::env::temp_dir().join(format!(
        "echode-audition-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    Fixture(directory)
}

/// 16-bit PCM WAV with interleaved `frames`.
fn wav(rate: u32, channels: u16, frames: &[i16]) -> Vec<u8> {
    let size = (frames.len() * 2) as u32;
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + size).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(channels.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * u32::from(channels) * 2).to_le_bytes());
    wav.extend((channels * 2).to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(size.to_le_bytes());
    for sample in frames {
        wav.extend(sample.to_le_bytes());
    }
    wav
}

fn tone(frames: usize, channels: usize, step: f32, scale: f32) -> Vec<i16> {
    (0..frames * channels)
        .map(|index| {
            let channel = index % channels;
            let frame = index / channels;
            let sign = if channel == 0 { 1.0 } else { -0.5 };
            (sign * scale * (frame as f32 * step).sin() * 32_767.0) as i16
        })
        .collect()
}

#[derive(Clone, Default)]
struct Probe {
    audio: Arc<Mutex<Option<AuditionAudio>>>,
    notes: Arc<Mutex<Option<StartNotes>>>,
    opens: Arc<AtomicUsize>,
    asked: Arc<Mutex<Vec<Option<String>>>>,
    broken: Arc<AtomicBool>,
    /// While set, `prepare` waits after noting `held`.
    hold: Arc<AtomicBool>,
    held: Arc<AtomicBool>,
    fail_open: Arc<AtomicBool>,
    /// What `panic` reports while the output is broken.
    panic: Arc<Mutex<Option<String>>>,
}

struct TestOutputs {
    probe: Probe,
    rate: u32,
    control: Option<(Option<String>, AuditionControl, HeardNotes)>,
}

impl AuditionOutputs for TestOutputs {
    fn prepare(&mut self, uid: Option<&str>) -> Result<(u32, bool), AudioError> {
        if self.probe.fail_open.load(Ordering::Acquire) {
            return Err(AudioError::NoOutputDevice);
        }
        lock(&self.probe.asked).push(uid.map(str::to_owned));
        while self.probe.hold.load(Ordering::Acquire) {
            self.probe.held.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(1));
        }
        if self.healthy()
            && let Some((asked, _, _)) = &self.control
            && asked.as_deref() == uid
        {
            return Ok((self.rate, false));
        }
        let (control, audio) = audition_pair(self.rate)?;
        let (notes, heard) = heard_notes();
        *lock(&self.probe.audio) = Some(audio);
        *lock(&self.probe.notes) = Some(notes);
        self.probe.broken.store(false, Ordering::Release);
        self.probe.opens.fetch_add(1, Ordering::AcqRel);
        self.control = Some((uid.map(str::to_owned), control, heard));
        Ok((self.rate, true))
    }
    fn control(&mut self) -> Option<&mut AuditionControl> {
        self.control.as_mut().map(|(_, control, _)| control)
    }
    fn is_open(&self) -> bool {
        self.control.is_some()
    }
    fn healthy(&self) -> bool {
        self.control.is_some() && !self.probe.broken.load(Ordering::Acquire)
    }
    fn panic(&self) -> Option<String> {
        lock(&self.probe.panic).clone()
    }
    fn heard(&mut self) -> Option<(u64, u64)> {
        self.control
            .as_mut()
            .and_then(|(_, _, heard)| heard.latest())
    }
    fn close(&mut self) {
        self.control = None;
    }
}

/// The first `ExtAudioFileOpenURL` of a newly linked test binary took more
/// than 10 s on 2026-11-03 while macOS set AudioToolbox up for it; the next
/// run of the same binary did not. That is not audition time, so it is spent
/// once here, before any test starts its stall deadline.
fn warm_audio_toolbox() {
    static WARM: std::sync::Once = std::sync::Once::new();
    WARM.call_once(|| {
        let fixture = fixture("warm");
        let sample = fixture.0.join("warm.wav");
        std::fs::write(&sample, wav(RATE, 1, &[0; 16])).unwrap();
        let started = Instant::now();
        echode_history::AudioFileReader::open(&sample).expect("AudioToolbox opens a WAV");
        eprintln!("AudioToolbox's first open took {:?}", started.elapsed());
    });
}

fn audition(rate: u32) -> (Audition, Probe) {
    warm_audio_toolbox();
    let probe = Probe::default();
    let outputs = probe.clone();
    let audition = Audition::spawn(move || {
        Box::new(TestOutputs {
            probe: outputs,
            rate,
            control: None,
        })
    })
    .unwrap();
    (audition, probe)
}

fn wait(audition: &Audition, what: impl Fn(&AuditionReport) -> bool) -> AuditionReport {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let report = audition.report();
        if what(&report) {
            return report;
        }
        assert!(Instant::now() < deadline, "audition stalled at {report:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Renders interleaved stereo through the product callback.
fn hear(callback: &AuditionCallback, frames: usize, block: usize) -> Vec<[f32; 2]> {
    let mut heard = Vec::with_capacity(frames);
    let mut interleaved = vec![0.0f32; block * 2];
    while heard.len() < frames {
        let count = block.min(frames - heard.len());
        let data = &mut interleaved[..count * 2];
        let mut buffers = [AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: std::mem::size_of_val(data) as u32,
            mData: data.as_mut_ptr().cast(),
        }];
        // SAFETY: reads the host clock and takes no pointers.
        let host = unsafe { AudioGetCurrentHostTime() };
        // SAFETY: the test calls this in place of HAL, never alongside it, and each buffer points
        // at a local array writable for its byte size.
        unsafe { fill(callback, &mut buffers, host) };
        heard.extend(data.chunks_exact(2).map(|pair| [pair[0], pair[1]]));
    }
    heard
}

fn callback(probe: &Probe, block: usize) -> AuditionCallback {
    let audio = lock(&probe.audio)
        .take()
        .expect("an opened audition output");
    let notes = lock(&probe.notes).take().expect("its start notes");
    AuditionCallback::new(audio, notes, block, 2, Arc::new(AtomicU64::new(0)), 0)
}

fn planes(path: &std::path::Path, rate: u32) -> (Vec<f32>, Vec<f32>) {
    let pcm = echode_history::decode_audio_file(path, &|| true).unwrap();
    let planes = crate::clips::device_planes(&pcm, rate).unwrap();
    let right = planes.right.unwrap_or_else(|| Arc::clone(&planes.left));
    (planes.left.to_vec(), right.to_vec())
}

fn fade(age: usize) -> f32 {
    if age >= FADE - 1 {
        0.0
    } else {
        (0.5 * (1.0 + (std::f64::consts::PI * age as f64 / (FADE - 1) as f64).cos())) as f32
    }
}

fn close(heard: [f32; 2], expected: [f32; 2], what: &str) {
    for channel in 0..2 {
        assert!(
            (heard[channel] - expected[channel]).abs() < 1.0e-6,
            "{what}: {heard:?} != {expected:?}"
        );
    }
}

#[test]
fn contract_a_long_audition_is_handed_over_before_the_file_finishes_decoding_under_rtsan() {
    let fixture = fixture("streaming");
    let sample = fixture.0.join("long.wav");
    // This fixture exceeds the audition's bounded read-ahead, without opening a device.
    std::fs::write(
        &sample,
        wav(RATE, 2, &tone(RATE as usize * 4, 2, 0.021, 0.5)),
    )
    .unwrap();
    let (audition, probe) = audition(RATE);
    let request = audition.play(sample);
    let report = wait(&audition, |report| report.phase == AuditionPhase::Sounding);
    let entry = report.history.back().unwrap();
    assert_eq!(entry.request, request);
    assert!(entry.handed_us.is_some());
    assert!(
        entry.decoded_us.is_none(),
        "the whole file delayed the first sound"
    );
    let waveform = report.waveform.as_ref().unwrap();
    assert_eq!(
        (waveform.frames, waveform.sample_rate),
        (u64::from(RATE) * 4, RATE)
    );
    assert!(!waveform.complete && waveform.loaded_frames < waveform.frames);
    assert!(!waveform.peaks.is_empty() && waveform.peaks.len() <= 512);
    assert!(
        waveform
            .peaks
            .iter()
            .any(|peak| peak[0] < 0.0 && peak[1] > 0.0)
    );
    let callback = callback(&probe, 128);
    assert!(hear(&callback, 256, 128).iter().any(|pair| pair[0] != 0.0));
    let stopped = audition.stop();
    wait(&audition, |report| report.handled == stopped);
    assert!(
        hear(&callback, 512, 128)
            .iter()
            .skip(FADE)
            .all(|pair| *pair == [0.0; 2])
    );
    assert!(audition.report().waveform.is_none());

    // A replacement clears the old overview before any decoding can finish.
    let missing = audition.play(fixture.0.join("missing.wav"));
    let report = wait(&audition, |report| report.handled == missing);
    assert_eq!(report.phase, AuditionPhase::Failed);
    assert!(report.waveform.is_none());
}

#[test]
#[ignore = "measures audition startup without an audio device"]
fn measure_streaming_audition_startup_without_a_device() {
    let fixture = fixture("startup-measurement");
    let mut rows = Vec::new();
    // These durations compare startup on this machine only; they do not limit files.
    for seconds in [4, 60] {
        let sample = fixture.0.join(format!("{seconds}s.wav"));
        std::fs::write(
            &sample,
            wav(RATE, 2, &tone(RATE as usize * seconds, 2, 0.021, 0.5)),
        )
        .unwrap();
        let (audition, probe) = audition(RATE);
        let request = audition.play(sample);
        wait(&audition, |report| report.phase == AuditionPhase::Sounding);
        let callback = callback(&probe, 128);
        hear(&callback, 128, 128);
        wait(&audition, |report| {
            report.history.back().unwrap().heard_us.is_some()
        });
        let measured = entry(&audition, request);
        assert!(measured.decoded_us.is_none());
        rows.push(serde_json::json!({"seconds": seconds, "stages": measured}));
        let stopped = audition.stop();
        wait(&audition, |report| report.handled == stopped);
    }
    let target =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/spike-audition.json");
    std::fs::write(target, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
}

#[test]
fn contract_selected_sample_is_heard_raw_once_and_the_newest_selection_wins_under_rtsan() {
    let fixture = fixture("contract");
    let stereo = fixture.0.join("stereo.wav");
    let mono = fixture.0.join("mono 44k.wav");
    let text = fixture.0.join("notes.wav");
    std::fs::write(&stereo, wav(RATE, 2, &tone(6_000, 2, 0.021, 0.5))).unwrap();
    std::fs::write(&mono, wav(44_100, 1, &tone(2_000, 1, 0.05, 0.4))).unwrap();
    std::fs::write(&text, b"not audio").unwrap();
    let (stereo_left, stereo_right) = planes(&stereo, RATE);
    let (mono_left, _) = planes(&mono, RATE);
    let (audition, probe) = audition(RATE);
    let block = 128;

    // The file's own samples, unchanged and from the first one.
    let first = audition.play(stereo.clone());
    let report = wait(&audition, |report| report.phase == AuditionPhase::Sounding);
    assert_eq!(report.request, first);
    assert_eq!(report.sample_rate, Some(RATE));
    let callback = callback(&probe, block);
    let heard = hear(&callback, 1_000, block);
    for (index, pair) in heard.iter().enumerate() {
        assert_eq!(
            *pair,
            [stereo_left[index], stereo_right[index]],
            "frame {index}"
        );
    }

    // A newer selection starts at its first sample; the older one fades out.
    let second = audition.play(mono.clone());
    wait(&audition, |report| {
        report.request == second && report.phase == AuditionPhase::Sounding
    });
    let heard = hear(&callback, mono_left.len() + 500, block);
    for (index, pair) in heard.iter().enumerate() {
        let new = mono_left.get(index).copied().unwrap_or(0.0);
        let gain = fade(index);
        let old = [
            stereo_left[1_000 + index] * gain,
            stereo_right[1_000 + index] * gain,
        ];
        close(
            *pair,
            [new + old[0], new + old[1]],
            &format!("replacement {index}"),
        );
    }
    wait(&audition, |report| report.phase == AuditionPhase::Ended);

    // Unreadable files fail without sound.
    let failed = audition.play(text);
    let report = wait(&audition, |report| report.handled == failed);
    assert_eq!(report.phase, AuditionPhase::Failed);
    assert!(report.error.is_some());
    assert!(
        hear(&callback, 512, block)
            .iter()
            .all(|pair| *pair == [0.0; 2])
    );

    // Two selections before the thread or callback catch up: only the second is heard.
    audition.play(stereo.clone());
    let last = audition.play(mono.clone());
    wait(&audition, |report| {
        report.request == last && report.phase == AuditionPhase::Sounding
    });
    let heard = hear(&callback, 256, block);
    for (index, pair) in heard.iter().enumerate() {
        assert_eq!(*pair, [mono_left[index]; 2], "only the newest at {index}");
    }

    // Stop fades the heard sound; nothing follows it.
    let stopped = audition.stop();
    assert_eq!(audition.report().phase, AuditionPhase::Stopped);
    wait(&audition, |report| report.handled == stopped);
    let heard = hear(&callback, 1_024, block);
    for (index, pair) in heard.iter().enumerate() {
        let expected = mono_left.get(256 + index).copied().unwrap_or(0.0) * fade(index);
        close(*pair, [expected; 2], &format!("stop {index}"));
    }
    assert_eq!(audition.report().request, stopped);
    assert_eq!(probe.opens.load(Ordering::Acquire), 1);
    drop(callback);
}

#[test]
fn the_audition_follows_the_project_output_and_closes_when_the_device_changes() {
    let fixture = fixture("output");
    let sample = fixture.0.join("long.wav");
    std::fs::write(&sample, wav(RATE, 1, &tone(RATE as usize, 1, 0.02, 0.3))).unwrap();
    let (audition, probe) = audition(RATE);
    audition.play(sample.clone());
    wait(&audition, |report| report.phase == AuditionPhase::Sounding);
    audition.follow_output(Some("external".into()));
    let request = audition.play(sample.clone());
    wait(&audition, |report| {
        report.request == request && report.phase == AuditionPhase::Sounding
    });
    assert_eq!(*lock(&probe.asked), [None, Some("external".to_owned())]);
    assert_eq!(probe.opens.load(Ordering::Acquire), 2);
    probe.broken.store(true, Ordering::Release);
    let report = wait(&audition, |report| report.sample_rate.is_none());
    assert_eq!(report.phase, AuditionPhase::Stopped);
    assert!(report.error.is_some());
    // The next selection opens the output again.
    let request = audition.play(sample);
    wait(&audition, |report| {
        report.request == request && report.phase == AuditionPhase::Sounding
    });
    assert_eq!(probe.opens.load(Ordering::Acquire), 3);
}

/// XCA-14 (ADR 0238): a panicked callback stops the sound with where it
/// panicked, not as a device change.
#[test]
fn a_panicked_audition_callback_stops_its_sound_with_the_reason() {
    let fixture = fixture("panic");
    let sample = fixture.0.join("long.wav");
    std::fs::write(&sample, wav(RATE, 1, &tone(RATE as usize, 1, 0.02, 0.3))).unwrap();
    let (audition, probe) = audition(RATE);
    let request = audition.play(sample);
    wait(&audition, |report| {
        report.request == request && report.phase == AuditionPhase::Sounding
    });
    *lock(&probe.panic) = Some("audition.rs:1:1: injected".into());
    probe.broken.store(true, Ordering::Release);
    let report = wait(&audition, |report| report.phase == AuditionPhase::Stopped);
    assert_eq!(
        report.error.as_deref(),
        Some("the audition callback panicked: audition.rs:1:1: injected")
    );
}

fn entry(audition: &Audition, request: u64) -> AuditionEntry {
    audition
        .report()
        .history
        .into_iter()
        .find(|entry| entry.request == request)
        .expect("a history entry")
}

#[test]
fn the_history_times_each_stage_and_keeps_the_input_that_asked() {
    let fixture = fixture("history");
    let sample = fixture.0.join("short.wav");
    std::fs::write(&sample, wav(44_100, 1, &tone(2_000, 1, 0.05, 0.4))).unwrap();
    let (audition, probe) = audition(RATE);
    let click = AuditionOrigin {
        input: AuditionInput::Click,
        delay_us: Some(1_500),
    };
    let first = audition.play_with(sample.clone(), click);
    wait(&audition, |report| report.phase == AuditionPhase::Sounding);
    let callback = callback(&probe, 128);
    hear(&callback, 128, 128);
    // The thread reads the callback's note while it services the output.
    let deadline = Instant::now() + Duration::from_secs(10);
    let timed = loop {
        let timed = entry(&audition, first);
        if timed.heard_us.is_some() {
            break timed;
        }
        assert!(Instant::now() < deadline, "no start time in {timed:?}");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(
        (timed.input, timed.input_delay_us),
        (AuditionInput::Click, Some(1_500))
    );
    assert_eq!(timed.path, Some(sample.display().to_string()));
    assert_eq!(
        (timed.source_frames, timed.source_rate),
        (Some(2_000), Some(44_100))
    );
    assert!(timed.opened_output && timed.requested_at_ms > 0);
    let stages = [
        timed.picked_us,
        timed.output_us,
        timed.first_decoded_us,
        timed.converted_us,
        timed.handed_us,
        timed.heard_us,
    ]
    .map(|stage| stage.expect("every stage is timed"));
    assert!(stages.is_sorted(), "{timed:?}");

    // A request the thread passes over is marked, and is never handed over.
    probe.hold.store(true, Ordering::Release);
    let key = AuditionOrigin {
        input: AuditionInput::Key,
        delay_us: Some(40),
    };
    let held = audition.play_with(sample.clone(), key);
    while !probe.held.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let newest = audition.play_with(sample.clone(), key);
    probe.hold.store(false, Ordering::Release);
    wait(&audition, |report| {
        report.request == newest && report.phase == AuditionPhase::Sounding
    });
    let passed = entry(&audition, held);
    assert_eq!(passed.phase, AuditionPhase::Superseded, "{passed:?}");
    assert!(
        passed.output_us.is_some()
            && passed.first_decoded_us.is_none()
            && passed.handed_us.is_none()
    );
    let kept = entry(&audition, newest);
    assert_eq!(kept.input, AuditionInput::Key);
    // The output was still open from the first request.
    assert!(kept.output_us.is_some() && !kept.opened_output);

    // A stop is recorded with its input. The record stays bounded.
    let stopped = audition.stop_with(click);
    wait(&audition, |report| report.handled == stopped);
    let stop = entry(&audition, stopped);
    assert_eq!(
        (stop.phase, stop.path.as_deref()),
        (AuditionPhase::Stopped, None)
    );
    assert!(stop.picked_us.is_some() && stop.decoded_us.is_none());
    for _ in 0..HISTORY {
        audition.stop();
    }
    let report = audition.report();
    assert_eq!(report.history.len(), HISTORY);
    assert_eq!(
        report.history.back().map(|entry| entry.request),
        Some(report.request)
    );
    assert!(report.history.iter().all(|entry| entry.request > stopped));
    let value = serde_json::to_value(entry(&audition, report.request)).unwrap();
    assert_eq!(value["input"], "other");
    assert!(value.get("requested").is_none());
    drop(callback);
}

#[test]
fn the_callback_notes_each_new_sound_once_with_its_block_time_under_rtsan() {
    let (mut control, audio) = audition_pair(RATE).unwrap();
    let (notes, mut heard) = heard_notes();
    let callback = AuditionCallback::new(audio, notes, 64, 2, Arc::new(AtomicU64::new(0)), 0);
    let mut data = vec![0.0f32; 64 * 2];
    let mut run = |host: u64| {
        let mut buffers = [AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: std::mem::size_of_val(data.as_slice()) as u32,
            mData: data.as_mut_ptr().cast(),
        }];
        // SAFETY: the test calls this in place of HAL, never alongside it, and each buffer points
        // at a local array writable for its byte size.
        unsafe { fill(&callback, &mut buffers, host) };
    };
    // SAFETY: reads the host clock and takes no pointers.
    let now = unsafe { AudioGetCurrentHostTime() };
    run(now);
    assert_eq!(heard.latest(), None);
    // Shorter than a block: it ends where it starts, and is still noted.
    let short = control.play(AuditionSound::new(Arc::from([0.25f32; 8]), None).unwrap());
    run(now + 10);
    run(now + 20);
    let (sound, at) = heard.latest().unwrap();
    assert_eq!(sound, short);
    // SAFETY: the conversion reads the host clock's timebase and takes no pointers.
    assert_eq!(at, unsafe { AudioConvertHostTimeToNanos(now + 10) });
    // HAL without a host time leaves the time unknown, not wrong.
    let next = control.play(AuditionSound::new(Arc::from([0.25f32; 8]), None).unwrap());
    run(0);
    assert_eq!(heard.latest().map(|(sound, _)| sound), Some(short));
    assert_ne!(next, short);
}

#[test]
fn the_callback_writes_every_layout_and_is_silent_after_a_device_change_under_rtsan() {
    let frames = 64;
    let sound: Arc<[f32]> = (0..frames * 4).map(|index| index as f32 / 1024.0).collect();
    for layout in 0..3 {
        let (mut control, audio) = audition_pair(RATE).unwrap();
        let epoch = Arc::new(AtomicU64::new(0));
        let channels = if layout == 2 { 1 } else { 2 };
        let (notes, _heard) = heard_notes();
        let callback = AuditionCallback::new(audio, notes, frames, channels, Arc::clone(&epoch), 0);
        let right: Arc<[f32]> = sound.iter().map(|sample| -sample).collect();
        control.play(AuditionSound::new(Arc::clone(&sound), Some(right)).unwrap());
        let mut interleaved = vec![1.0f32; frames * channels as usize];
        let mut planar = [vec![1.0f32; frames], vec![1.0f32; frames]];
        let mut run = || -> Vec<[f32; 2]> {
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
                    mNumberChannels: channels,
                    mDataByteSize: std::mem::size_of_val(interleaved.as_slice()) as u32,
                    mData: interleaved.as_mut_ptr().cast(),
                }]
            };
            // SAFETY: the test calls this in place of HAL, never alongside it, and each buffer
            // points at a local array writable for its byte size.
            unsafe { fill(&callback, &mut buffers, 0) };
            (0..frames)
                .map(|frame| match layout {
                    1 => [planar[0][frame], planar[1][frame]],
                    2 => [interleaved[frame]; 2],
                    _ => [interleaved[frame * 2], interleaved[frame * 2 + 1]],
                })
                .collect()
        };
        for (frame, pair) in run().iter().enumerate() {
            let (left, right) = (sound[frame], -sound[frame]);
            let expected = if layout == 2 {
                [(left + right) * std::f32::consts::FRAC_1_SQRT_2; 2]
            } else {
                [left, right]
            };
            assert_eq!(*pair, expected, "layout {layout} frame {frame}");
        }
        epoch.fetch_add(1, Ordering::AcqRel);
        assert!(run().iter().all(|pair| *pair == [0.0; 2]));
        assert!(!callback.failed());
    }
}

#[test]
#[ignore = "needs a CoreAudio output; make spike-audio runs the silent probe"]
fn a_silent_audition_opens_the_system_output_and_ends() {
    let fixture = fixture("hal");
    let silent = fixture.0.join("silent.wav");
    std::fs::write(&silent, wav(44_100, 2, &vec![0; 44_100 / 5 * 2])).unwrap();
    let audition = Audition::start().unwrap();
    let request = audition.play(silent);
    let report = wait(&audition, |report| {
        report.request == request
            && matches!(report.phase, AuditionPhase::Ended | AuditionPhase::Failed)
    });
    assert_eq!(report.phase, AuditionPhase::Ended, "{report:?}");
    assert!(report.sample_rate.is_some());
}

#[test]
fn contract_loading_is_not_an_idle_output_but_stopping_allows_idle_close() {
    let shared = Shared::default();
    lock(&shared.report).phase = AuditionPhase::Loading;
    let mut outputs = TestOutputs {
        probe: Probe::default(),
        rate: RATE,
        control: None,
    };
    outputs.prepare(None).unwrap();
    let mut sounding = None;
    // Inject an expired idle clock; this is a state boundary, not a timing measurement.
    let expired = Instant::now() - KEEP_OPEN - Duration::from_secs(1);
    let mut idle_since = Some(expired);
    service(&shared, &mut outputs, &mut sounding, &mut idle_since);
    assert!(outputs.is_open(), "loading must not expire the idle output");
    assert!(idle_since.is_none());
    lock(&shared.report).phase = AuditionPhase::Stopped;
    idle_since = Some(expired);
    service(&shared, &mut outputs, &mut sounding, &mut idle_since);
    assert!(!outputs.is_open());
}

#[test]
fn contract_a_visible_browser_keeps_its_silent_output_until_hidden() {
    let shared = Shared::default();
    lock(&shared.report).settings.visible = true;
    let mut outputs = TestOutputs {
        probe: Probe::default(),
        rate: RATE,
        control: None,
    };
    outputs.prepare(None).unwrap();
    let mut sounding = None;
    // Expired state exercises idle expiry without measuring elapsed time.
    let expired = Instant::now() - KEEP_OPEN - Duration::from_secs(1);
    let mut idle_since = Some(expired);
    service(&shared, &mut outputs, &mut sounding, &mut idle_since);
    assert!(
        outputs.is_open(),
        "a visible browser must retain its silent output"
    );
    lock(&shared.report).settings.visible = false;
    idle_since = Some(expired);
    service(&shared, &mut outputs, &mut sounding, &mut idle_since);
    assert!(!outputs.is_open());
}

#[test]
fn browser_visibility_prepares_silence_without_a_play_and_settings_do_not_cancel_loading_under_rtsan()
 {
    let (audition, probe) = audition(RATE);
    audition.configure(true, 0.25).unwrap();
    let report = wait(&audition, |report| report.sample_rate == Some(RATE));
    assert_eq!((report.request, report.phase), (0, AuditionPhase::Idle));
    assert!(report.history.is_empty() && report.output_error.is_none());
    let callback = callback(&probe, 64);
    assert!(hear(&callback, 64, 64).iter().all(|pair| *pair == [0.0; 2]));
    let fixture = fixture("settings");
    let sample = fixture.0.join("long.wav");
    std::fs::write(&sample, wav(RATE, 1, &vec![8192; RATE as usize * 2])).unwrap();
    let request = audition.play(sample);
    wait(&audition, |report| {
        report.request == request && report.phase == AuditionPhase::Sounding
    });
    assert_eq!(hear(&callback, 1, 1)[0], [0.0625; 2]);
    audition.configure(true, 0.0).unwrap();
    // Wait for the control setting, without consuming the whole read-ahead.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let silent = hear(&callback, 64, 64);
        if silent.last() == Some(&[0.0; 2]) {
            break;
        }
        assert!(Instant::now() < deadline, "audition gain was not applied");
        std::thread::sleep(SERVICE);
    }
    let report = audition.report();
    assert_eq!(report.request, request);
    assert_eq!(probe.opens.load(Ordering::Acquire), 1);
    assert_eq!(report.settings.gain, 0.0);
    for invalid in [f32::NAN, f32::INFINITY, -1.0, 1.1] {
        assert!(matches!(
            audition.configure(false, invalid),
            Err(AudioError::Rejected(Rejected::AuditionGain))
        ));
    }
    assert!(audition.report().settings.visible);
    audition.stop();
    audition.hide();
}

#[test]
fn contract_sample_position_and_volume_reach_the_product_callback_under_rtsan() {
    let fixture = fixture("position");
    let sample = fixture.0.join("offset.wav");
    // The source and blocks cover this seek/EOF fixture only.
    let source = tone(1631, 2, 0.021, 0.5);
    std::fs::write(&sample, wav(44_100, 2, &source)).unwrap();
    for rate in [44_100, 48_000] {
        for requested_start in [137u64, 1600, u64::MAX] {
            let start = requested_start.min(1630);
            let suffix = echode_history::DecodedPcm {
                sample_rate: 44_100,
                channels: 2,
                frames: 1631 - start,
                samples: source[start as usize * 2..]
                    .iter()
                    .map(|s| f32::from(*s) / 32768.0)
                    .collect(),
            };
            let expected = crate::clips::device_planes(&suffix, rate).unwrap();
            for block in [1, 17, 256] {
                let (audition, probe) = audition(rate);
                audition.configure(false, 0.25).unwrap();
                let request =
                    audition.play_from(sample.clone(), requested_start, AuditionOrigin::default());
                let report = wait(&audition, |r| {
                    r.request == request && r.phase == AuditionPhase::Sounding
                });
                let waveform = report.waveform.unwrap();
                assert_eq!(
                    (
                        waveform.frames,
                        waveform.first_frame,
                        waveform.loaded_frames
                    ),
                    (1631, start, 1631)
                );
                assert_eq!(report.history.back().unwrap().start_frame, start);
                let callback = callback(&probe, block);
                let heard = hear(&callback, expected.left.len() + 1, block);
                for (index, frame) in heard.iter().take(expected.left.len()).enumerate() {
                    close(
                        *frame,
                        [
                            expected.left[index] * 0.25,
                            expected.right.as_ref().unwrap()[index] * 0.25,
                        ],
                        "seek and volume",
                    );
                }
                assert_eq!(heard.last(), Some(&[0.0; 2]));
            }
        }
    }
    // Seeking at EOF is legal; seeking beyond it is rejected by the decoder.
    let mut reader = echode_history::AudioFileReader::open(&sample).unwrap();
    reader.seek(1631).unwrap();
    assert_eq!(reader.read(17).unwrap().frames, 0);
    assert!(reader.seek(1632).is_err());
    assert!(reader.seek(u64::MAX).is_err());
    reader.seek(137).unwrap();
    assert_eq!(
        reader.read(1).unwrap().samples[0],
        f32::from(source[274]) / 32768.0
    );
}

#[test]
fn silent_output_errors_are_reported_and_visibility_can_retry_without_playing() {
    let probe = Probe::default();
    probe.fail_open.store(true, Ordering::Release);
    let outputs = probe.clone();
    let audition = Audition::spawn(move || {
        Box::new(TestOutputs {
            probe: outputs,
            rate: RATE,
            control: None,
        })
    })
    .unwrap();
    audition.configure(true, 0.5).unwrap();
    let report = wait(&audition, |r| r.output_error.is_some());
    assert_eq!(
        (report.request, report.phase, report.sample_rate),
        (0, AuditionPhase::Idle, None)
    );
    probe.fail_open.store(false, Ordering::Release);
    audition.configure(true, 0.5).unwrap();
    wait(&audition, |r| {
        r.sample_rate.is_some() && r.output_error.is_none()
    });
    probe.broken.store(true, Ordering::Release);
    wait(&audition, |r| {
        r.sample_rate.is_some() && probe.opens.load(Ordering::Acquire) == 2
    });
    assert_eq!(audition.report().request, 0);
    audition.follow_output(Some("external".into()));
    wait(&audition, |r| {
        r.sample_rate.is_some() && probe.opens.load(Ordering::Acquire) == 3
    });
    assert_eq!(
        lock(&probe.asked).last().cloned(),
        Some(Some("external".into()))
    );
    audition.hide();
}

#[test]
fn a_seek_to_a_suffix_shorter_than_a_device_frame_ends_normally() {
    let fixture = fixture("tiny-suffix");
    let sample = fixture.0.join("high-rate.wav");
    std::fs::write(&sample, wav(192_000, 1, &[8192; 100])).unwrap();
    let (audition, _) = audition(RATE);
    let request = audition.play_from(sample, u64::MAX, AuditionOrigin::default());
    let report = wait(&audition, |r| r.handled == request);
    assert_eq!(report.phase, AuditionPhase::Ended);
    assert!(report.error.is_none());
    assert_eq!(report.history.back().unwrap().start_frame, 99);
}

/// The browser's overview reads the whole file without an output, matches the
/// peaks a finished audition reports, and stops between reads once a newer
/// selection asks (ADR 0163). Three reads' worth of frames cross read edges.
#[test]
fn contract_a_sample_overview_reads_the_whole_file_without_an_audition() {
    let dir = fixture("overview");
    let frames = 65_536 * 2 + 1_234;
    let path = dir.0.join("long.wav");
    std::fs::write(&path, wav(RATE, 2, &tone(frames, 2, 0.003, 0.8))).unwrap();
    let overview = sample_overview(&path, &|| true).unwrap().unwrap();
    assert!(overview.complete);
    assert_eq!(overview.frames, frames as u64);
    assert_eq!(overview.loaded_frames, frames as u64);
    assert_eq!(overview.first_frame, 0);
    assert_eq!(overview.sample_rate, RATE);
    assert_eq!(
        overview.peaks.len() as u64,
        overview.frames.div_ceil(overview.frames_per_peak)
    );
    assert!(overview.peaks.len() <= 512);
    // Left peaks near +0.8, right near -0.4 at its lowest: both channels count.
    let high = overview
        .peaks
        .iter()
        .map(|peak| peak[1])
        .fold(f32::MIN, f32::max);
    let low = overview
        .peaks
        .iter()
        .map(|peak| peak[0])
        .fold(f32::MAX, f32::min);
    assert!(high > 0.79 && high <= 0.8001, "{high}");
    assert!(low < -0.79, "{low}");

    // A short file, which an audition reads to the end before it sounds,
    // gets the same overview from both.
    let short = dir.0.join("short.wav");
    std::fs::write(&short, wav(RATE, 2, &tone(12_000, 2, 0.01, 0.5))).unwrap();
    let (audition, _probe) = audition(RATE);
    audition.play(short.clone());
    let report = wait(&audition, |report| {
        report
            .waveform
            .as_ref()
            .is_some_and(|waveform| waveform.complete)
    });
    assert_eq!(report.waveform, sample_overview(&short, &|| true).unwrap());

    // A newer request stops this one before the next read.
    let reads = AtomicUsize::new(0);
    let stopped = sample_overview(&path, &|| reads.fetch_add(1, Ordering::SeqCst) < 1).unwrap();
    assert!(stopped.is_none());
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert!(sample_overview(&dir.0.join("missing.wav"), &|| true).is_err());
}
