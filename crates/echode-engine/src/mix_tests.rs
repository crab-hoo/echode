use super::*;

#[test]
fn center_pan_keeps_power_and_hard_pan_picks_one_side() {
    let plane = [1.0, -0.5];
    let mut left = [0.0; 2];
    let mut right = [0.0; 2];
    let mut peaks = [0.0; 1];
    let master = mix_tracks(
        &[&plane],
        &[TrackFader::unity()],
        &mut left,
        &mut right,
        &mut peaks,
    )
    .unwrap();
    assert!((left[0] - right[0]).abs() < 1e-6);
    assert!((left[0] * left[0] + right[0] * right[0] - 1.0).abs() < 1e-5);
    assert!((peaks[0] - left[0].abs()).abs() < 1e-6);
    assert!((master - peaks[0]).abs() < 1e-6);

    let hard_left = TrackFader {
        pan: -1.0,
        ..TrackFader::unity()
    };
    mix_tracks(&[&plane], &[hard_left], &mut left, &mut right, &mut peaks).unwrap();
    assert_eq!(left[0], 1.0);
    assert_eq!(right[0], 0.0);
    assert_eq!(left[1], -0.5);
}

#[test]
fn a_centered_stereo_track_keeps_its_sides_and_a_hard_left_drops_the_right() {
    let source_left = [0.5, -0.25];
    let source_right = [0.125, 0.75];
    let channels: [&[f32]; 2] = [&source_left, &source_right];
    let track = InputBus {
        layout: STEREO,
        channels: &channels,
    };
    let mut left = [0.0; 2];
    let mut right = [0.0; 2];
    let mut peaks = [0.0; 1];
    {
        let mut master: [&mut [f32]; 2] = [&mut left, &mut right];
        mix_buses(&[track], &[TrackFader::unity()], &mut master, &mut peaks).unwrap();
    }
    assert_eq!(left, source_left);
    assert_eq!(right, source_right);

    let hard_left = TrackFader {
        pan: -1.0,
        gain: 0.5,
        ..TrackFader::unity()
    };
    {
        let mut master: [&mut [f32]; 2] = [&mut left, &mut right];
        mix_buses(&[track], &[hard_left], &mut master, &mut peaks).unwrap();
    }
    assert_eq!(left, [0.25, -0.125]);
    assert_eq!(right, [0.0, 0.0]);
    assert!((peaks[0] - 0.25).abs() < 1e-6);

    let odd_layout = [Speaker::Left, Speaker::Right, Speaker::Mono];
    let odd_channels: [&[f32]; 3] = [&source_left, &source_right, &source_left];
    let odd = InputBus {
        layout: &odd_layout,
        channels: &odd_channels,
    };
    let mut master: [&mut [f32]; 2] = [&mut left, &mut right];
    assert!(matches!(
        mix_buses(&[odd], &[TrackFader::unity()], &mut master, &mut peaks).unwrap_err(),
        PlaybackError::BufferTooSmall
    ));
}

#[test]
fn mute_and_solo_choose_who_reaches_the_master() {
    let a = [1.0];
    let b = [1.0];
    let mut left = [0.0; 1];
    let mut right = [0.0; 1];
    let mut peaks = [0.0; 2];
    let muted = TrackFader {
        mute: true,
        pan: -1.0,
        ..TrackFader::unity()
    };
    let open = TrackFader {
        pan: 1.0,
        ..TrackFader::unity()
    };
    mix_tracks(&[&a, &b], &[muted, open], &mut left, &mut right, &mut peaks).unwrap();
    assert_eq!(left[0], 0.0);
    assert_eq!(right[0], 1.0);
    assert_eq!(peaks[0], 0.0);

    let solo_muted = TrackFader {
        mute: true,
        solo: true,
        pan: -1.0,
        ..TrackFader::unity()
    };
    let other = TrackFader {
        pan: 1.0,
        ..TrackFader::unity()
    };
    let master = mix_tracks(
        &[&a, &b],
        &[solo_muted, other],
        &mut left,
        &mut right,
        &mut peaks,
    )
    .unwrap();
    assert_eq!(master, 0.0);

    let solo = TrackFader {
        solo: true,
        pan: -1.0,
        ..TrackFader::unity()
    };
    mix_tracks(&[&a, &b], &[solo, other], &mut left, &mut right, &mut peaks).unwrap();
    assert_eq!(left[0], 1.0);
    assert_eq!(right[0], 0.0);
    assert_eq!(peaks[1], 0.0);
}

#[test]
fn two_tracks_sum_and_a_bad_fader_is_rejected() {
    let a = [0.25];
    let b = [0.25];
    let mut left = [0.0; 1];
    let mut right = [0.0; 1];
    let mut peaks = [0.0; 2];
    let half = TrackFader {
        gain: 0.5,
        pan: -1.0,
        ..TrackFader::unity()
    };
    mix_tracks(&[&a, &b], &[half, half], &mut left, &mut right, &mut peaks).unwrap();
    assert!((left[0] - 0.25).abs() < 1e-6);
    assert_eq!(right[0], 0.0);

    let wild = TrackFader {
        pan: 1.5,
        ..TrackFader::unity()
    };
    assert!(matches!(
        mix_tracks(&[&a], &[wild], &mut left, &mut right, &mut peaks[..1]).unwrap_err(),
        PlaybackError::InvalidFader
    ));
    assert!(matches!(
        mix_tracks(&[&a, &b], &[half], &mut left, &mut right, &mut peaks).unwrap_err(),
        PlaybackError::BufferTooSmall
    ));
}
