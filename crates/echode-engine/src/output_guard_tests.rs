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
