//! Dry host bypass. Buffers are prepared on control; this only copies samples.

/// Copies input planes without changing their gain or channel order. Missing
/// input is silence, including a bypassed instrument before any upstream audio.
/// Plane counts and capacities belong to the caller, not to a fixed device cap.
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
pub fn copy_bypassed_audio<'a>(
    mut input: impl Iterator<Item = &'a [f32]>,
    output: &mut [Vec<f32>],
    frames: usize,
) {
    for plane in output {
        let source = input.next().unwrap_or_default();
        let end = frames.min(plane.len());
        let copied = end.min(source.len());
        let (Some((copy, silence)), Some(source)) = (
            plane
                .get_mut(..end)
                .and_then(|plane| plane.split_at_mut_checked(copied)),
            source.get(..copied),
        ) else {
            continue;
        };
        copy.copy_from_slice(source);
        silence.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bypass_copy_and_silence_do_not_allocate_under_rtsan() {
        // These three short planes are only this regression's scratch space.
        let input = [vec![0.25; 19], vec![-0.5; 19], vec![0.75; 19]];
        let mut output = vec![vec![0.0; 19]; 3];
        {
            let _guard = crate::ForbidAlloc::enter();
            super::copy_bypassed_audio(input.iter().map(Vec::as_slice), &mut output, 19);
        }
        assert_eq!(output, input);
        {
            let _guard = crate::ForbidAlloc::enter();
            super::copy_bypassed_audio(std::iter::empty(), &mut output, 19);
        }
        assert!(output.iter().flatten().all(|value| *value == 0.0));
    }
}
