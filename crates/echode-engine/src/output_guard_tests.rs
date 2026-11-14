use super::*;

#[test]
fn both_device_outputs_share_the_exact_boundary_and_nonfinite_rule() {
    assert!(first_fault(&[GUARD, -GUARD], &[-GUARD, GUARD]).is_none());
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 8.0, -8.0] {
        for (left, right) in [([0.25, bad], [-0.5, 0.0]), ([0.25, 0.0], [-0.5, bad])] {
            let (frame, peak) = first_fault(&left, &right).unwrap();
            assert_eq!(frame, 1);
            if bad.is_nan() {
                assert!(peak.is_nan());
            } else {
                assert_eq!(peak, bad.abs());
            }
        }
    }
    assert!(first_fault(&[], &[]).is_none());
}

#[test]
fn a_narrowed_sum_faults_only_where_it_is_not_finite() {
    // The largest finite values are a level, not a fault, without a guard.
    assert!(first_nonfinite(&[f32::MAX, -f32::MAX], &[-f32::MAX, GUARD * 2.0]).is_none());
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for (left, right) in [
            ([f32::MAX, bad], [-0.5, 0.0]),
            ([0.25, 0.0], [-f32::MAX, bad]),
        ] {
            let (frame, peak) = first_nonfinite(&left, &right).unwrap();
            assert_eq!(frame, 1);
            assert!(peak.is_nan() == bad.is_nan() && (bad.is_nan() || peak == f32::INFINITY));
        }
    }
    // What `as` makes of an f64 sum past the f32 range.
    let narrowed = [1.0, 4.0e38f64 as f32];
    assert_eq!(
        first_nonfinite(&narrowed, &[0.0, 0.0]),
        Some((1, f32::INFINITY))
    );
}
