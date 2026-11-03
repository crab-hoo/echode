//! The same missing-output intervals must sound identical across partitions.

use echode_engine::{ForbidAlloc, PluginDeclick};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;

fn render(rate: u32, block: usize) -> Vec<[f32; 2]> {
    let fade = (rate / 200).max(2) as usize;
    let mut declick = PluginDeclick::new(rate);
    let mut heard = Vec::new();
    // These segment lengths and scratch planes describe only this regression.
    let mut left = vec![0.0; block];
    let mut right = left.clone();
    for (length, missed) in [
        (17, false),
        (fade / 3, true),
        (fade / 4, false),
        (fade * 2, true),
        (fade * 2, false),
    ] {
        let before = heard.last().copied();
        let start = heard.len();
        for offset in (0..length).step_by(block) {
            let count = block.min(length - offset);
            left[..count].fill(0.75);
            right[..count].fill(-0.25);
            {
                let _guard = ForbidAlloc::enter();
                declick.process(&mut left[..count], &mut right[..count], missed);
            }
            heard.extend(left[..count].iter().zip(&right).map(|(&l, &r)| [l, r]));
        }
        if let Some(before) = before {
            assert_eq!(
                heard[start], before,
                "a transition must start at the last emitted sample"
            );
        }
        if missed && length >= fade {
            assert_eq!(heard.last().copied(), Some([0.0; 2]));
        }
    }
    assert_eq!(heard.last().copied(), Some([0.75, -0.25]));
    heard
}

#[test]
fn plugin_deadline_fades_are_sample_counted_even_when_retargeted_under_rtsan() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let reference = render(rate, 1);
        for block in [7, 31, 64, 512, 1024] {
            assert_eq!(render(rate, block), reference, "{rate} Hz / {block} frames");
        }
        let fade = (rate / 200) as usize;
        let bound = 0.75 / (fade - 1) as f32 + 1.0e-6;
        assert!(reference.windows(2).all(|pair| {
            (pair[1][0] - pair[0][0]).abs() <= bound && (pair[1][1] - pair[0][1]).abs() <= bound
        }));
    }
}

#[test]
fn healthy_plugin_waveforms_are_unchanged_and_invalid_samples_do_not_poison_a_join_under_rtsan() {
    let mut declick = PluginDeclick::new(48_000);
    let mut left = [0.0, 0.5, -0.75, f32::MAX];
    let mut right = [0.25, -0.125, 0.5, -f32::MAX];
    let expected = (left, right);
    declick.process(&mut left, &mut right, false);
    assert_eq!(
        (left, right),
        expected,
        "healthy note onsets must not be faded"
    );
    declick.process(&mut left, &mut right, true);
    left.fill(f32::INFINITY);
    right.fill(f32::NAN);
    declick.process(&mut left, &mut right, false);
    assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
    declick.reset();
    declick.process(&mut left, &mut right, false);
    assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
}

#[test]
fn cold_misses_empty_buffers_and_removing_a_chain_do_not_leave_old_audio_under_rtsan() {
    // A two-frame fade exercises the minimum at tiny sample rates.
    let mut declick = PluginDeclick::new(1);
    let (mut left, mut right) = ([0.75; 2], [-0.25; 2]);
    declick.process(&mut left, &mut right, true);
    assert_eq!((left, right), ([0.0; 2], [0.0; 2]));
    left.fill(0.75);
    right.fill(-0.25);
    declick.process(&mut [], &mut [], false);
    declick.process(&mut left, &mut right[..1], false);
    declick.process(&mut left, &mut right, false);
    assert_eq!((left, right), ([0.0, 0.75], [0.0, -0.25]));
    declick.process(&mut left[..1], &mut right[..1], true);
    declick.reset();
    left.fill(0.125);
    right.fill(0.25);
    declick.process(&mut left, &mut right, false);
    assert_eq!((left, right), ([0.125; 2], [0.25; 2]));
}
