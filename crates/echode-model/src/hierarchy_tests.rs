use super::*;
use std::collections::HashSet;

fn track(id: usize, parent: Option<usize>) -> Arc<Track> {
    // These fixtures exercise only parent links; mixers and arrange keys are
    // validated separately through the product entry points.
    Arc::new(Track {
        id: Id::from_raw(id as u128),
        name: format!("track {id}"),
        parent: parent.map(|id| Id::from_raw(id as u128)),
        order: crate::initial_order(),
        devices: Vec::new(),
        clips: Vec::new(),
    })
}

fn scalar_forest(tracks: &[Arc<Track>]) -> bool {
    tracks.iter().all(|track| {
        let mut seen = HashSet::new();
        let mut cursor = Some(track.id);
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return false;
            }
            let Some(track) = tracks.iter().find(|track| track.id == id) else {
                return false;
            };
            cursor = track.parent;
        }
        true
    })
}

#[test]
fn forest_matches_scalar_walk_for_all_small_parent_graphs_in_either_order() {
    // Four nodes bound exhaustive enumeration here, not product track counts.
    // Every parent is either absent or one of the nodes, including self-links.
    for count in 0..=4_usize {
        let base = count + 1;
        for mut code in 0..base.pow(count as u32) {
            let mut tracks: Vec<_> = (1..=count)
                .map(|id| {
                    let digit = code % base;
                    code /= base;
                    track(id, (digit != 0).then_some(digit))
                })
                .collect();
            let expected = scalar_forest(&tracks);
            for _ in 0..2 {
                let result = check_forest(&tracks);
                assert_eq!(result.is_ok(), expected, "{tracks:?}: {result:?}");
                if !expected {
                    assert!(matches!(result, Err(ForestError::Cycle { .. })));
                }
                tracks.reverse();
            }
        }
    }
}

#[test]
fn missing_parent_is_reported_even_at_the_end_of_a_chain() {
    let tracks = vec![track(1, Some(2)), track(2, Some(3)), track(3, Some(99))];
    assert_eq!(
        check_forest(&tracks),
        Err(ForestError::MissingParent {
            track: Id::from_raw(1),
            parent: Id::from_raw(99),
        })
    );
}

#[test]
fn reverse_ordered_deep_chain_needs_no_recursion() {
    // This depth exercises the iterative traversal, not a product limit.
    let tracks: Vec<_> = (1..=4_096)
        .rev()
        .map(|id| track(id, (id > 1).then_some(id - 1)))
        .collect();
    check_forest(&tracks).unwrap();
}
