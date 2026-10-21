use super::*;

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
