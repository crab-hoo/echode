use super::*;

fn float_wav(samples: &[f32]) -> Vec<u8> {
    let size = std::mem::size_of_val(samples) as u32;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((36 + size).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(3u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(RATE.to_le_bytes());
    bytes.extend((RATE * 4).to_le_bytes());
    bytes.extend(4u16.to_le_bytes());
    bytes.extend(32u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(size.to_le_bytes());
    for sample in samples {
        bytes.extend(sample.to_le_bytes());
    }
    bytes
}

#[test]
fn contract_audition_guard_reports_a_fault_and_releases_stream_backpressure_under_rtsan() {
    // Short EOF and a source longer than the read-ahead ring exercise both
    // completion and a decoder waiting for room. These are fixture sizes only.
    for frames in [512, RATE as usize * 4] {
        let dir = fixture("guard");
        let bad = dir.0.join("bad.wav");
        let good = dir.0.join("good.wav");
        let mut samples = vec![100.0; frames];
        samples[..70].fill(0.25);
        std::fs::write(&bad, float_wav(&samples)).unwrap();
        std::fs::write(&good, wav(RATE, 1, &[8192; 512])).unwrap();
        let (audition, probe) = audition(RATE);
        let request = audition.play(bad);
        wait(&audition, |report| report.phase == AuditionPhase::Sounding);
        let callback = callback(&probe, 128);
        let heard = hear(&callback, 512, 128);
        assert_eq!(&heard[..70], &[[0.25; 2]; 70]);
        assert!(heard[70..].iter().flatten().all(|s| *s == 0.0));
        let report = wait(&audition, |report| report.phase == AuditionPhase::Failed);
        assert_eq!(report.request, request);
        assert_eq!(
            report.error.as_deref(),
            Some(AuditionFault::OutputGuard.to_string().as_str())
        );
        assert!(report.sample_rate.is_some(), "the output stays reusable");
        let entry = report.history.back().unwrap();
        assert_eq!(entry.phase, AuditionPhase::Failed);
        assert_eq!(entry.error, report.error);
        let heard = hear(&callback, 512, 128);
        assert!(heard.iter().flatten().all(|s| *s == 0.0));
        assert_eq!(audition.report().phase, AuditionPhase::Failed);

        // A settings refresh does not change the selection gate. It can be
        // handled only after the faulting stream leaves its backpressure loop.
        let preparations = lock(&probe.asked).len();
        audition.configure(true, 1.0).unwrap();
        wait(&audition, |_| lock(&probe.asked).len() > preparations);

        // Reusing the output must not wait for the old decoder or replay its fade.
        let next = audition.play(good);
        wait(&audition, |report| {
            report.request == next && report.phase == AuditionPhase::Sounding
        });
        let heard = hear(&callback, 512, 128);
        assert_eq!(heard, vec![[0.25; 2]; 512]);
        let report = wait(&audition, |report| report.phase == AuditionPhase::Ended);
        assert_eq!(report.request, next);
        assert!(report.error.is_none());
        assert_eq!(probe.opens.load(Ordering::Acquire), 1);
        assert_eq!(report.history.front().unwrap().phase, AuditionPhase::Failed);
    }
}

#[test]
fn a_mono_audition_guards_the_folded_value_before_writing_the_hal_buffer_under_rtsan() {
    let (mut control, audio) = audition_pair(RATE).unwrap();
    let id = control.play(AuditionSound::new(vec![6.0; 128].into(), None).unwrap());
    let (notes, _) = heard_notes();
    // This output shape belongs to the fixture, not to a product channel limit.
    let callback = AuditionCallback::new(audio, notes, 128, 1, Arc::new(AtomicU64::new(0)), 0);
    let mut samples = [0.0f32; 128];
    let mut buffers = [AudioBuffer {
        mNumberChannels: 1,
        mDataByteSize: std::mem::size_of_val(&samples) as u32,
        mData: samples.as_mut_ptr().cast(),
    }];
    // SAFETY: this test exclusively calls the callback with its owned writable buffer.
    unsafe { fill(&callback, &mut buffers, 0) };
    assert_eq!(samples, [0.0; 128]);
    assert_eq!(control.auto_muted(), Some(id));
    control.service();
    let good = control.play(AuditionSound::new(vec![0.25; 128].into(), None).unwrap());
    // SAFETY: same exclusively owned callback and writable buffer as above.
    unsafe { fill(&callback, &mut buffers, 0) };
    assert_eq!(samples, [0.5 * std::f32::consts::FRAC_1_SQRT_2; 128]);
    assert_ne!(control.auto_muted(), Some(good));
}
