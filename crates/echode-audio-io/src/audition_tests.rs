use super::*;
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
}

struct TestOutputs {
    probe: Probe,
    rate: u32,
    control: Option<(Option<String>, AuditionControl, HeardNotes)>,
}

impl AuditionOutputs for TestOutputs {
    fn prepare(&mut self, uid: Option<&str>) -> Result<(u32, bool), String> {
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
        let (control, audio) = audition_pair(self.rate).map_err(|error| error.to_string())?;
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
    fn heard(&mut self) -> Option<(u64, u64)> {
        self.control
            .as_mut()
            .and_then(|(_, _, heard)| heard.latest())
    }
    fn close(&mut self) {
        self.control = None;
    }
}

fn audition(rate: u32) -> (Audition, Probe) {
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
        timed.decoded_us,
        timed.output_us,
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
    assert!(passed.decoded_us.is_some() && passed.handed_us.is_none());
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
