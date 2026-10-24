//! Bounded source reads, continuous SRC and progressive peaks for auditions.

use super::*;
use echode_engine::ResampleStream;
use echode_history::AudioFileReader;

/// Display resolution, not a limit on file length or sound storage.
const PEAKS: u64 = 512;

pub(super) fn start(
    shared: &Shared,
    outputs: &mut dyn AuditionOutputs,
    request: u64,
    path: &std::path::Path,
    start_frame: u64,
    alive: &Weak<()>,
) -> Option<u64> {
    let mut sounding = None;
    let result = load(
        shared,
        outputs,
        request,
        path,
        start_frame,
        alive,
        &mut sounding,
    );
    if let Err(error) = result {
        if let Some(control) = outputs.control() {
            control.stop();
        }
        if !outputs.is_open() {
            shared.set_rate(None);
        }
        let stopped = lock(&shared.report).phase == AuditionPhase::Stopped;
        if !stopped {
            shared.update(request, AuditionPhase::Failed, Some(error));
        }
        return None;
    }
    sounding.map(|(_, id)| id)
}

fn load(
    shared: &Shared,
    outputs: &mut dyn AuditionOutputs,
    request: u64,
    path: &std::path::Path,
    start_frame: u64,
    alive: &Weak<()>,
    sounding: &mut Option<(u64, u64)>,
) -> Result<(), String> {
    let current = || shared.current(request) && alive.strong_count() != 0;
    if !current() {
        return Ok(());
    }
    let mut reader = AudioFileReader::open(path).map_err(|error| error.to_string())?;
    let start_frame = start_frame.min(reader.frames().saturating_sub(1));
    reader
        .seek(start_frame)
        .map_err(|error| error.to_string())?;
    shared.stage(request, |entry, _| {
        entry.start_frame = start_frame;
        entry.source_frames = Some(reader.frames());
        entry.source_rate = Some(reader.sample_rate());
    });
    if !current() {
        return Ok(());
    }
    let uid = lock(&shared.output).clone();
    let (rate, opened) = outputs.prepare(uid.as_deref())?;
    shared.set_rate(Some(rate));
    lock(&shared.report).output_error = None;
    shared.stage(request, |entry, at| {
        entry.output_us = Some(at);
        entry.opened_output = opened;
    });
    if !current() {
        return Ok(());
    }
    let mut converter = ResampleStream::new(reader.sample_rate(), rate, reader.channels())
        .map_err(|error| error.to_string())?;
    // Half a second is a bounded handoff window, not a limit on sample duration.
    let (mut writer, sound) =
        AuditionSound::streaming((rate as usize / 2).max(1)).map_err(|error| error.to_string())?;
    let mut waiting = Some(sound);
    let mut supplied = 0usize;
    let mut waveform = AuditionWaveform {
        frames: reader.frames(),
        loaded_frames: start_frame,
        first_frame: start_frame,
        sample_rate: reader.sample_rate(),
        frames_per_peak: reader.frames().div_ceil(PEAKS).max(1),
        ..AuditionWaveform::default()
    };
    let mut idle_since = None;
    loop {
        if !current() {
            return Ok(());
        }
        let ask = u32::try_from(converter.input_frames()).map_err(|error| error.to_string())?;
        let pcm = reader.read(ask).map_err(|error| error.to_string())?;
        shared.stage(request, |entry, at| {
            entry.first_decoded_us.get_or_insert(at);
        });
        let eof = pcm.frames < u64::from(ask);
        add_peaks(&mut waveform, &pcm);
        waveform.complete = eof;
        if eof {
            waveform.frames = waveform.loaded_frames;
            shared.stage(request, |entry, at| entry.decoded_us = Some(at));
        }
        {
            let mut report = lock(&shared.report);
            if report.request == request {
                report.waveform = Some(waveform.clone());
            }
        }
        let frames = converter
            .process(&pcm.samples, eof)
            .map_err(|error| error.to_string())?;
        if !frames.is_empty() {
            shared.stage(request, |entry, at| {
                entry.converted_us.get_or_insert(at);
            });
        }
        for frame in frames {
            loop {
                if !current() {
                    return Ok(());
                }
                if writer.push(frame).map_err(|error| error.to_string())? {
                    break;
                }
                service(shared, outputs, sounding, &mut idle_since);
                if !outputs.is_open() {
                    return Err("the audition output closed during loading".into());
                }
                // Backpressure waits only on this control thread. Selection and
                // document closure cancel it without waiting for the callback.
                std::thread::sleep(Duration::from_millis(1));
            }
            supplied += 1;
            // Start after 100 ms of read-ahead; shorter files start at EOF.
            if supplied >= (rate as usize / 10).max(1)
                && reader.frames() - start_frame > u64::from(reader.sample_rate() / 2)
            {
                hand_over(shared, outputs, request, &mut waiting, sounding)?;
            }
        }
        if eof {
            if supplied == 0 {
                if start_frame > 0 {
                    // A sub-frame suffix may round to zero at a lower device
                    // rate. It ends normally, and must still replace old audio.
                    if let Some(control) = outputs.control() {
                        control.stop();
                    }
                    shared.update(request, AuditionPhase::Ended, None);
                    return Ok(());
                }
                return Err("audio file has no output samples".into());
            }
            // EOF is published before the callback can see a short sound.
            drop(writer);
            hand_over(shared, outputs, request, &mut waiting, sounding)?;
            return Ok(());
        }
        service(shared, outputs, sounding, &mut idle_since);
        if !outputs.is_open() {
            return Err("the audition output closed during loading".into());
        }
    }
}

fn hand_over(
    shared: &Shared,
    outputs: &mut dyn AuditionOutputs,
    request: u64,
    waiting: &mut Option<AuditionSound>,
    sounding: &mut Option<(u64, u64)>,
) -> Result<(), String> {
    if let Some(sound) = waiting.take() {
        if !shared.current(request) {
            return Ok(());
        }
        let control = outputs.control().ok_or("no audition output is open")?;
        let _ = control.set_gain(lock(&shared.report).settings.gain);
        let id = control.play(sound);
        *sounding = Some((request, id));
        shared.stage(request, |entry, at| entry.handed_us = Some(at));
        shared.update(request, AuditionPhase::Sounding, None);
        shared.handled(request);
    }
    Ok(())
}

fn add_peaks(waveform: &mut AuditionWaveform, pcm: &echode_history::DecodedPcm) {
    for samples in pcm.samples.chunks_exact(usize::from(pcm.channels)) {
        let at = (waveform.loaded_frames / waveform.frames_per_peak).min(PEAKS - 1) as usize;
        if waveform.peaks.len() <= at {
            waveform.peaks.resize(at + 1, [0.0; 2]);
        }
        if let Some(peak) = waveform.peaks.get_mut(at) {
            for &sample in samples {
                peak[0] = peak[0].min(sample);
                peak[1] = peak[1].max(sample);
            }
        }
        waveform.loaded_frames += 1;
    }
}
