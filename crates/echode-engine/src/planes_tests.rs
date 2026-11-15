use super::*;

// The planes below bound these checks only; the type takes any length.

#[test]
fn planes_refuse_a_non_finite_sample_at_its_earliest_frame() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut left = vec![0.25; 8];
        left[5] = bad;
        let mut right = vec![0.25; 8];
        right[3] = bad;
        assert_eq!(
            FinitePlanes::new(left.clone().into(), None).unwrap_err(),
            PlaneFault::NonFinite { frame: 5 }
        );
        assert_eq!(
            FinitePlanes::new(left.into(), Some(right.clone().into())).unwrap_err(),
            PlaneFault::NonFinite { frame: 3 }
        );
        assert_eq!(
            FinitePlanes::new(vec![0.25; 8].into(), Some(right.into())).unwrap_err(),
            PlaneFault::NonFinite { frame: 3 }
        );
    }
}

#[test]
fn planes_refuse_a_stereo_pair_of_two_lengths() {
    assert_eq!(
        FinitePlanes::new(vec![0.25; 8].into(), Some(vec![0.25; 7].into())).unwrap_err(),
        PlaneFault::Lengths
    );
}

#[test]
fn planes_keep_the_largest_finite_magnitude_of_both_planes() {
    let planes = FinitePlanes::new(
        vec![0.25, -f32::MAX, 0.5].into(),
        Some(vec![0.0, 0.0, 0.75].into()),
    )
    .unwrap();
    assert_eq!(planes.peak(), f32::MAX);
    assert_eq!(planes.frames(), 3);
    let mono = FinitePlanes::new(vec![-0.5, 0.25].into(), None).unwrap();
    assert_eq!(mono.peak(), 0.5);
    assert!(mono.right().is_none());
    assert_eq!(FinitePlanes::new(Arc::from([]), None).unwrap().peak(), 0.0);
}

#[test]
fn a_window_is_judged_by_the_frames_it_plays() {
    let planes = FinitePlanes::new(
        vec![0.25, 0.25, 0.5, -f32::MAX].into(),
        Some(vec![0.0, -0.75, 0.0, 0.0].into()),
    )
    .unwrap();
    let head = PlacedPlanes::windowed(planes.clone(), ClipWindow { lead: 0, frames: 3 });
    assert_eq!(head.peak(), 0.75);
    assert!(head.allows_gain(2.0));
    // A negative lead and a tail past the planes read silence there.
    let around = PlacedPlanes::windowed(
        planes.clone(),
        ClipWindow {
            lead: -4,
            frames: 6,
        },
    );
    assert_eq!(around.peak(), 0.75);
    let outside = PlacedPlanes::windowed(planes.clone(), ClipWindow { lead: 9, frames: 4 });
    assert_eq!(outside.peak(), 0.0);
    assert!(outside.allows_gain(f32::MAX));
    let whole = PlacedPlanes::from(planes.clone());
    assert_eq!(whole.peak(), f32::MAX);
    assert!(whole.allows_gain(1.0));
    assert!(!whole.allows_gain(2.0));
    // The window that is the planes is the whole placement.
    let same = PlacedPlanes::windowed(planes, ClipWindow { lead: 0, frames: 4 });
    assert!(same.window().is_none());
    assert_eq!(same.played(), ClipWindow { lead: 0, frames: 4 });
}

#[test]
fn a_placement_reads_its_window_and_silence_around_it() {
    let planes = FinitePlanes::new(vec![0.1, 0.2, 0.3].into(), None).unwrap();
    let placed = PlacedPlanes::windowed(
        planes,
        ClipWindow {
            lead: -1,
            frames: 5,
        },
    );
    let left = placed.planes().left().clone();
    let read: Vec<f32> = (0..6).map(|index| placed.sample(&left, index)).collect();
    assert_eq!(read, [0.0, 0.1, 0.2, 0.3, 0.0, 0.0]);
}
