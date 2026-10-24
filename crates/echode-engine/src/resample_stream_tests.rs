use super::*;

#[test]
fn incremental_conversion_matches_whole_file_conversion_including_the_last_sample() {
    // These lengths exercise full, partial and shorter-than-delay fixtures only.
    for (source, output) in [
        (48_000, 48_000),
        (44_100, 48_000),
        (96_000, 44_100),
        (8_000, 192_000),
    ] {
        for length in [1, 33, 1024, 2048, 30_017] {
            let left: Vec<_> = (0..length)
                .map(|i| (i as f32 * 0.031).sin() * 0.4)
                .collect();
            let input: Vec<_> = left.iter().flat_map(|l| [*l, -*l * 0.5]).collect();
            let frames =
                (length as u64 * u64::from(output) + u64::from(source / 2)) / u64::from(source);
            let mut expected = vec![0.0; frames as usize];
            crate::resample_into(&left, source, &mut expected, output).unwrap();
            let mut stream = ResampleStream::new(source, output, 2).unwrap();
            let chunk = stream.input_frames() * 2;
            let mut heard = Vec::new();
            let chunks = input.chunks(chunk);
            for (index, samples) in chunks.enumerate() {
                heard.extend(
                    stream
                        .process(samples, (index + 1) * chunk >= input.len())
                        .unwrap(),
                );
            }
            assert_eq!(
                heard.len(),
                expected.len(),
                "{source}->{output}, length {length}"
            );
            for (index, (actual, expected)) in heard.iter().zip(expected).enumerate() {
                assert!(
                    (actual[0] - expected).abs() < 1e-6,
                    "{source}->{output} frame {index}"
                );
                assert!((actual[1] + expected * 0.5).abs() < 1e-6);
            }
        }
    }
}

#[test]
fn the_stream_rejects_bad_rates_layouts_partial_reads_and_nonfinite_samples() {
    assert!(ResampleStream::new(0, 48_000, 1).is_err());
    assert!(ResampleStream::new(48_000, 0, 1).is_err());
    assert!(ResampleStream::new(48_000, 48_000, 3).is_err());
    let mut stream = ResampleStream::new(44_100, 48_000, 2).unwrap();
    assert!(stream.process(&[0.0], true).is_err());
    assert!(stream.process(&[0.0; 4], false).is_err());
    assert!(stream.process(&[f32::NAN; 4], true).is_err());
    stream.process(&[0.0; 4], true).unwrap();
    assert!(stream.process(&[], true).is_err());
}
