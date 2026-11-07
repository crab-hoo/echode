//! Compare frontier discovery with the original iterative rule on valid windows.

use super::*;

fn clip(slot: usize, start: i64, length: i64, kind: ClipKind) -> Placement {
    let mut clip = Placement::new_auto(
        Id::from_raw(slot as u128 * 2 + 1),
        Id::from_raw(slot as u128 * 2 + 2),
        Tick::new(start),
        Tick::new(length),
    );
    clip.kind = kind;
    clip
}

// Deliberately retain the slow rule and use i128 arithmetic, not the production
// interval helpers, to check fixed walls, inclusive ends and integer limits.
fn scalar_hosts(clips: &[Placement], index: usize, span: (i64, i64), bar: i64) -> Vec<usize> {
    let range = |clip: &Placement| {
        let start = i128::from(clip.start.get());
        let last = start + i128::from(clip.length.get()) - 1;
        (
            start.div_euclid(i128::from(bar)),
            last.div_euclid(i128::from(bar)),
        )
    };
    let mut span = (i128::from(span.0), i128::from(span.1));
    let mut selected = vec![false; clips.len()];
    selected[index] = true;
    loop {
        let mut changed = false;
        for (slot, clip) in clips.iter().enumerate() {
            if selected[slot] || clip.kind != ClipKind::Auto {
                continue;
            }
            let candidate = range(clip);
            if span.0 > candidate.1 + 1 || candidate.0 > span.1 + 1 {
                continue;
            }
            let union = (span.0.min(candidate.0), span.1.max(candidate.1));
            if clips.iter().any(|clip| {
                let fixed = range(clip);
                clip.kind == ClipKind::Fixed && union.0 <= fixed.1 && fixed.0 <= union.1
            }) {
                continue;
            }
            selected[slot] = true;
            span = union;
            changed = true;
        }
        if !changed {
            return selected
                .iter()
                .enumerate()
                .filter_map(|(slot, selected)| selected.then_some(slot))
                .collect();
        }
    }
}

#[test]
fn frontier_hosts_match_scalar_discovery_for_sorted_nonoverlapping_windows() {
    // Enumeration sizes and margins bound this regression only, not the product.
    for bar in [1, 4, 7] {
        for base in [-100, -3, 0, 50, i64::MIN + 128, i64::MAX - 128] {
            for pattern in 0..5 {
                for mask in 0..32 {
                    let mut start = base;
                    let clips: Vec<_> = (0..5)
                        .map(|slot| {
                            let length = 1 + (slot as i64 + pattern) % (2 * bar);
                            let kind = if mask & (1 << slot) == 0 {
                                ClipKind::Auto
                            } else {
                                ClipKind::Fixed
                            };
                            let clip = clip(slot, start, length, kind);
                            start += length + (slot as i64 * pattern) % (bar + 1);
                            clip
                        })
                        .collect();
                    for (index, seed) in clips.iter().enumerate() {
                        if seed.kind != ClipKind::Auto {
                            continue;
                        }
                        let origin = seed.start.get().div_euclid(bar);
                        for delta in [(-8, -8), (-2, -1), (-1, 1), (0, 0), (1, 1), (2, 8)] {
                            let span = (origin + delta.0, origin + delta.1);
                            let mut actual = refit_hosts(&clips, index, span, bar).unwrap();
                            actual.sort_unstable();
                            assert_eq!(
                                actual,
                                scalar_hosts(&clips, index, span, bar),
                                "bar={bar}, base={base}, pattern={pattern}, mask={mask}, index={index}, span={span:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn blocked_containing_window_does_not_hide_a_touching_window_on_its_left() {
    let clips = [
        clip(0, -7, 4, ClipKind::Auto),
        clip(1, -3, 44, ClipKind::Auto),
        clip(2, 41, 1, ClipKind::Fixed),
        clip(3, 48, 4, ClipKind::Auto),
    ];
    // The edited last clip's note moved to bar zero. The long candidate shares
    // its last bar with the wall, but the earlier window can still be absorbed.
    let mut hosts = refit_hosts(&clips, 3, (0, 0), 4).unwrap();
    hosts.sort_unstable();
    assert_eq!(hosts, vec![0, 3]);
    assert_eq!(hosts, scalar_hosts(&clips, 3, (0, 0), 4));
}
