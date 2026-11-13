use super::*;

fn ramp(frames: usize) -> (Vec<f32>, Vec<f32>) {
    let left = (1..=frames).map(|frame| frame as f32).collect();
    let right = (1..=frames).map(|frame| -(frame as f32)).collect();
    (left, right)
}

fn delayed(line: &mut Delay, left: &[f32], right: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut out_left, mut out_right) = (left.to_vec(), right.to_vec());
    for (left, right) in out_left.chunks_mut(block).zip(out_right.chunks_mut(block)) {
        line.process(left, Some(right));
    }
    (out_left, out_right)
}

#[test]
fn a_delay_shifts_both_planes_by_its_frames_for_every_block_size() {
    // These lengths bound this fixture only; a line holds any delay it can reserve.
    let (left, right) = ramp(300);
    for delay in [1usize, 7, 64, 65, 100] {
        // A planned block of 1 splits every larger block into parts.
        for (block, planned) in [1, 7, 64, 512]
            .into_iter()
            .flat_map(|block| [(block, block), (block, 1)])
        {
            let mut line = Delay::new(delay as u64, planned).unwrap();
            let (heard_left, heard_right) = delayed(&mut line, &left, &right, block);
            for frame in 0..left.len() {
                let expected = frame
                    .checked_sub(delay)
                    .map_or((0.0, 0.0), |source| (left[source], right[source]));
                assert_eq!(
                    (heard_left[frame], heard_right[frame]),
                    expected,
                    "delay {delay}, block {block} of {planned}, frame {frame}"
                );
            }
        }
    }
}

#[test]
fn a_zero_delay_has_no_buffer_and_passes_audio() {
    let mut line = Delay::new(0, 1).unwrap();
    let (mut left, mut right) = ramp(16);
    let (before_left, before_right) = (left.clone(), right.clone());
    line.process(&mut left, Some(&mut right));
    assert_eq!((left, right), (before_left, before_right));
    assert_eq!(line.ring.capacity(), 0);
}

#[test]
fn a_mono_block_becomes_the_right_history_for_a_later_stereo_block() {
    let mut line = Delay::new(4, 1).unwrap();
    let mut mono = [1.0, 2.0, 3.0, 4.0];
    line.process(&mut mono, None);
    assert_eq!(mono, [0.0; 4]);
    let (mut left, mut right) = ([0.0; 4], [0.0; 4]);
    line.process(&mut left, Some(&mut right));
    assert_eq!(left, [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(right, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn a_clear_silences_the_held_frames_without_touching_the_buffer() {
    let mut line = Delay::new(3, 1).unwrap();
    let mut first = [1.0, 2.0, 3.0];
    line.process(&mut first, None);
    line.clear();
    let mut next = [4.0, 5.0, 6.0, 7.0];
    line.process(&mut next, None);
    assert_eq!(next, [0.0, 0.0, 0.0, 4.0]);
}

#[test]
fn a_carried_line_continues_the_previous_history() {
    let (left, right) = ramp(40);
    // Same capacity: the buffers move. A shorter or longer delay within it
    // reads the history the previous plan wrote.
    for (before, after) in [
        (5u64, 5u64),
        (5, 3),
        (5, 7),
        (5, 20),
        (20, 5),
        (0, 6),
        (6, 0),
    ] {
        let mut previous = Delay::new(before, 1).unwrap();
        let (mut first_left, mut first_right) = (left[..20].to_vec(), right[..20].to_vec());
        previous.process(&mut first_left, Some(&mut first_right));
        let mut line = Delay::new(after, 1).unwrap();
        line.carry(&mut previous);
        let (mut rest_left, mut rest_right) = (left[20..].to_vec(), right[20..].to_vec());
        line.process(&mut rest_left, Some(&mut rest_right));
        let delay = after as usize;
        // The newest frames before the carry that the previous ring holds.
        let capacity = |frames: u64| (frames as usize + 1).next_power_of_two();
        let history = if before == 0 || after == 0 {
            0
        } else {
            capacity(before).min(20)
        };
        for (offset, (heard_left, heard_right)) in rest_left.iter().zip(&rest_right).enumerate() {
            let frame = 20 + offset;
            let expected = match frame.checked_sub(delay) {
                Some(source) if source >= 20 - history => (left[source], right[source]),
                _ => (0.0, 0.0),
            };
            assert_eq!(
                (*heard_left, *heard_right),
                expected,
                "{before} -> {after}, frame {frame}"
            );
        }
    }
}

#[test]
fn a_carry_moves_buffers_and_never_copies_a_history() {
    let mut previous = Delay::new(5, 1).unwrap();
    let mut line = Delay::new(6, 1).unwrap();
    let held = previous.ring.left.as_ptr();
    line.carry(&mut previous);
    assert_eq!(line.ring.left.as_ptr(), held, "equal capacities swap");
    assert_eq!(previous.ring.capacity(), line.ring.capacity());
    // A larger capacity reads the previous ring in place.
    let mut grown = Delay::new(1_000, 1).unwrap();
    grown.carry(&mut line);
    assert_eq!(
        grown.older.left.as_ptr(),
        held,
        "a changed capacity keeps the ring"
    );
    assert_eq!(line.ring.capacity(), 0);
}

#[test]
fn a_line_carried_twice_before_its_history_is_written_reads_what_it_kept() {
    let (left, right) = ramp(128);
    for regrow in [false, true] {
        let mut first = Delay::new(30, 1).unwrap();
        let (mut a, mut b) = (left[..32].to_vec(), right[..32].to_vec());
        first.process(&mut a, Some(&mut b));
        let mut second = Delay::new(40, 1).unwrap();
        second.carry(&mut first);
        let (mut a, mut b) = (left[32..34].to_vec(), right[32..34].to_vec());
        second.process(&mut a, Some(&mut b));
        // The same capacity moves both rings over. A changed capacity keeps
        // only the newest ring, so frames before the first carry are gone.
        let mut third = Delay::new(if regrow { 70 } else { 40 }, 1).unwrap();
        third.carry(&mut second);
        let delay = third.frames();
        let (mut a, mut b) = (left[34..].to_vec(), right[34..].to_vec());
        third.process(&mut a, Some(&mut b));
        for (offset, heard) in a.iter().enumerate() {
            let frame = 34 + offset;
            let kept = if regrow { 32 } else { 0 };
            let expected = match frame.checked_sub(delay) {
                Some(source) if source >= kept => left[source],
                _ => 0.0,
            };
            assert_eq!(*heard, expected, "regrow {regrow}, frame {frame}");
        }
        assert!(a.iter().any(|sample| *sample != 0.0));
    }
}

#[test]
fn a_clear_also_silences_the_older_ring() {
    let mut previous = Delay::new(3, 1).unwrap();
    let mut first = [1.0, 2.0, 3.0, 4.0];
    previous.process(&mut first, None);
    let mut line = Delay::new(20, 1).unwrap();
    line.carry(&mut previous);
    line.clear();
    let mut next = [5.0; 25];
    line.process(&mut next, None);
    assert!(next[..20].iter().all(|sample| *sample == 0.0));
    assert!(next[20..].iter().all(|sample| *sample == 5.0));
}

#[test]
fn an_unrepresentable_delay_is_an_error_not_an_abort() {
    assert!(matches!(
        Delay::new(u64::MAX, 1),
        Err(PlaybackError::Latency(u64::MAX))
    ));
}
