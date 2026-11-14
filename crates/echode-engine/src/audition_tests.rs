use super::*;

#[test]
fn the_guard_keeps_the_prefix_latches_and_recovers_without_an_old_tail_under_rtsan() {
    // These lengths exercise faults within and on block boundaries only.
    for block in [1, 7, 64, 512] {
        let (mut control, mut audio) = audition_pair(RATE).unwrap();
        let mut samples = vec![0.25; 1024];
        samples[70] = -100.0;
        let bad = control.play(AuditionSound::new(samples.into(), None).unwrap());
        let (left, right) = render(&mut audio, 1024, block);
        assert_eq!(&left[..70], &[0.25; 70]);
        assert_eq!(&right[..70], &[0.25; 70]);
        assert!(left[70..].iter().chain(&right[70..]).all(|s| *s == 0.0));
        assert_eq!(control.auto_muted(), Some(bad));
        assert_eq!(control.ended(), bad);
        assert!(control.heard().is_none());
        control.stop();
        let (left, right) = render(&mut audio, 512, block);
        assert!(left.iter().chain(&right).all(|s| *s == 0.0));
        control.service();
        assert!(control.idle());
        assert_eq!(control.auto_muted(), Some(bad));
        let good = control.play(AuditionSound::new(vec![0.5; 1024].into(), None).unwrap());
        let (left, right) = render(&mut audio, 512, block);
        assert_eq!(left, vec![0.5; 512]);
        assert_eq!(right, left);
        assert_eq!(control.heard(), Some(good));
        assert_ne!(control.auto_muted(), Some(good));
    }
}

#[test]
fn the_guard_checks_replacement_sums_and_the_mono_device_value_under_rtsan() {
    for mono in [false, true] {
        let (mut control, mut audio) = audition_pair(RATE).unwrap();
        let sound = || AuditionSound::new(vec![6.0; 1024].into(), None).unwrap();
        let first = control.play(sound());
        let (mut left, mut right) = ([0.0; 1], [0.0; 1]);
        if mono {
            audio.mix_mono(&mut left, &mut right);
            assert_eq!(left, [0.0]);
            assert_eq!(right, [0.0]);
            assert_eq!(control.auto_muted(), Some(first));
        } else {
            audio.mix(&mut left, &mut right);
            assert_eq!(left, [6.0]);
            assert_eq!(control.auto_muted(), None);
            let second = control.play(sound());
            left.fill(0.0);
            right.fill(0.0);
            audio.mix(&mut left, &mut right);
            assert_eq!(left, [0.0]);
            assert_eq!(right, [0.0]);
            assert_eq!(control.auto_muted(), Some(second));
        }
    }
}

#[test]
fn audition_gain_ramps_only_its_sound_and_can_retarget_without_a_jump_under_rtsan() {
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    control.set_gain(0.5).unwrap();
    // These constant frames isolate volume from source and fade changes.
    control
        .play(AuditionSound::new(vec![0.5; 3000].into(), Some(vec![-0.25; 3000].into())).unwrap());
    let (mut l, mut r) = ([0.125; 1], [-0.125; 1]);
    audio.mix(&mut l, &mut r);
    assert_eq!((l[0], r[0]), (0.375, -0.25));
    control.set_gain(0.0).unwrap();
    let (l, r) = render(&mut audio, FADE / 2, 17);
    assert!(l.windows(2).all(|p| p[1] < p[0]));
    assert!((l[FADE / 2 - 1] - 0.125).abs() < 1e-7);
    assert!(l.iter().zip(&r).all(|(l, r)| *l == -2.0 * r));
    control.set_gain(0.75).unwrap();
    let (l, _) = render(&mut audio, FADE, 17);
    assert!(l[0] > 0.125 && l[0] < 0.127);
    assert!(l.windows(2).all(|p| p[1] > p[0]));
    assert_eq!(l[FADE - 1], 0.375);
    for invalid in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(control.set_gain(invalid).is_err());
    }
    control.set_gain(0.0).unwrap();
    render(&mut audio, FADE, 64);
    let (mut l, mut r) = ([0.125; 64], [-0.125; 64]);
    audio.mix(&mut l, &mut r);
    assert_eq!(l, [0.125; 64]);
    assert_eq!(r, [-0.125; 64]);
}

#[test]
fn streamed_pcm_waits_through_underflow_and_drains_eof_without_freeing_on_audio_under_rtsan() {
    // Three frames bound this handoff fixture, not the length of a sound.
    let (mut writer, sound) = AuditionSound::streaming(3).unwrap();
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    for frame in [[0.1, -0.2], [0.3, -0.4], [0.5, -0.6]] {
        assert!(writer.push(frame).unwrap());
    }
    assert!(!writer.push([0.7, -0.8]).unwrap());
    let id = control.play(sound);
    let (mut left, mut right) = ([0.0; 5], [0.0; 5]);
    audio.mix(&mut left, &mut right);
    assert_eq!(left, [0.1, 0.3, 0.5, 0.0, 0.0]);
    assert_eq!(right, [-0.2, -0.4, -0.6, 0.0, 0.0]);
    assert_eq!(control.heard(), Some(id));
    assert_eq!(control.starved_frames(), 2);
    assert!(writer.push([0.7, -0.8]).unwrap());
    assert!(writer.push([f32::NAN, 0.0]).is_err());
    drop(writer);
    left.fill(0.0);
    right.fill(0.0);
    audio.mix(&mut left, &mut right);
    assert_eq!(left, [0.7, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(right, [-0.8, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(control.ended(), id);
    assert_eq!(control.starved_frames(), 2);
    assert!(!control.idle());
    control.service();
    assert!(control.idle());
    assert!(AuditionSound::streaming(0).is_err());
}

// The rate and lengths below shape these fixtures only.
const RATE: u32 = 48_000;
const FADE: usize = (RATE / 200) as usize;

fn ramp(frames: usize, scale: f32) -> Arc<[f32]> {
    (0..frames)
        .map(|index| scale * (index as f32 * 0.013 + 0.2).sin())
        .collect()
}

/// The product `mix` carries the RTSan scope; this helper allocates its output.
fn render(audio: &mut AuditionAudio, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    for (left, right) in left.chunks_mut(block).zip(right.chunks_mut(block)) {
        audio.mix(left, right);
    }
    (left, right)
}

fn gain(age: usize) -> f32 {
    stop_gain(age as u64, FADE as u32)
}

#[test]
fn a_stereo_sound_is_heard_unchanged_once_and_then_returned_under_rtsan() {
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    let left = ramp(1000, 0.5);
    let right = ramp(1000, -0.25);
    let id = control.play(AuditionSound::new(Arc::clone(&left), Some(Arc::clone(&right))).unwrap());
    let (heard_left, heard_right) = render(&mut audio, 1200, 64);
    assert_eq!(&heard_left[..1000], &left[..]);
    assert_eq!(&heard_right[..1000], &right[..]);
    assert!(
        heard_left[1000..]
            .iter()
            .chain(&heard_right[1000..])
            .all(|sample| *sample == 0.0)
    );
    assert_eq!(control.heard(), None);
    assert_eq!(control.ended(), id);
    assert!(!control.idle());
    control.service();
    assert!(control.idle());
}

#[test]
fn a_mono_sound_plays_on_both_channels_at_its_own_level() {
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    let mono = ramp(300, 0.75);
    let id = control.play(AuditionSound::new(Arc::clone(&mono), None).unwrap());
    let mut left = vec![0.1; 100];
    let mut right = vec![-0.1; 100];
    audio.mix(&mut left, &mut right);
    assert_eq!(control.heard(), Some(id));
    for index in 0..100 {
        assert_eq!(left[index], 0.1 + mono[index]);
        assert_eq!(right[index], -0.1 + mono[index]);
    }
}

#[test]
fn a_newer_sound_starts_at_its_first_sample_while_the_older_fades_under_rtsan() {
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    let old = ramp(4000, 0.5);
    let new = ramp(3000, -0.3);
    let first = control.play(AuditionSound::new(Arc::clone(&old), None).unwrap());
    render(&mut audio, 500, 100);
    let second = control.play(AuditionSound::new(Arc::clone(&new), None).unwrap());
    let (heard, _) = render(&mut audio, 1000, 100);
    for (index, sample) in heard.iter().enumerate() {
        let expected = new[index] + old.get(500 + index).copied().unwrap_or(0.0) * gain(index);
        assert_eq!(*sample, expected, "sample {index}");
    }
    assert!(gain(FADE - 1) == 0.0 && FADE < 1000);
    assert_eq!(control.heard(), Some(second));
    assert_eq!(control.ended(), first);
}

#[test]
fn stop_fades_the_heard_sound_and_a_waiting_sound_is_never_heard() {
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    let sound = ramp(4000, 0.5);
    let id = control.play(AuditionSound::new(Arc::clone(&sound), None).unwrap());
    render(&mut audio, 128, 128);
    control.stop();
    let (heard, _) = render(&mut audio, 512, 128);
    for (index, sample) in heard.iter().enumerate() {
        assert_eq!(*sample, sound[128 + index] * gain(index));
    }
    assert_eq!(control.heard(), None);
    assert_eq!(control.ended(), id);
    // A replaced sound that was never heard ends without a sample.
    control.play(AuditionSound::new(ramp(100, 1.0), None).unwrap());
    let last = control.play(AuditionSound::new(Arc::clone(&sound), None).unwrap());
    let (heard, _) = render(&mut audio, 64, 64);
    assert_eq!(&heard[..], &sound[..64]);
    assert_eq!(control.heard(), Some(last));
}

#[test]
fn requests_beyond_the_handoff_keep_only_the_newest_waiting() {
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    let sounds: Vec<_> = (0..IN_FLIGHT + 3)
        .map(|index| ramp(400, 0.1 * (index + 1) as f32))
        .collect();
    let mut ids = Vec::new();
    for sound in &sounds {
        ids.push(control.play(AuditionSound::new(Arc::clone(sound), None).unwrap()));
    }
    let newest = *ids.last().unwrap();
    let mut heard = Vec::new();
    for _ in 0..8 {
        render(&mut audio, 1, 1);
        heard.push(control.heard());
        control.service();
    }
    // The newest request reaches the callback once earlier sounds come back.
    // Requests that were replaced while waiting are never sent.
    assert!(heard.contains(&Some(newest)));
    for id in &ids[IN_FLIGHT..ids.len() - 1] {
        assert!(!heard.contains(&Some(*id)));
    }
    render(&mut audio, 1000, 1000);
    control.service();
    assert!(control.idle());
}

#[test]
fn non_finite_or_mismatched_planes_are_refused() {
    assert!(AuditionSound::new(Arc::from(vec![0.0, f32::NAN]), None).is_err());
    assert!(AuditionSound::new(Arc::from(vec![0.0; 3]), Some(Arc::from(vec![0.0; 2]))).is_err());
    assert!(AuditionSound::new(Arc::from(Vec::<f32>::new()), None).is_err());
    assert!(audition_pair(0).is_err());
}
