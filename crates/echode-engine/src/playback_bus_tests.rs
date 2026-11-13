use super::*;

/// root{a{a1, a2}, b}, top: positions in plan (pre)order.
fn tree() -> Bus {
    Bus::new(vec![None, Some(0), Some(1), Some(1), Some(0), None])
}

#[test]
fn children_and_roots_follow_plan_order() {
    let bus = tree();
    assert_eq!(bus.roots().collect::<Vec<_>>(), [0, 5]);
    assert_eq!(bus.children_of(0).collect::<Vec<_>>(), [1, 4]);
    assert_eq!(bus.children_of(1).collect::<Vec<_>>(), [2, 3]);
    assert!(bus.children_of(2).next().is_none());
    assert!(bus.has_children(0) && bus.has_children(1));
    assert!(!bus.has_children(2) && !bus.has_children(5));
    assert!(!bus.has_children(99));
}

#[test]
fn a_parent_is_ready_once_after_its_last_child_in_any_order() {
    let mut bus = tree();
    for _ in 0..2 {
        bus.begin();
        for position in [3, 5, 4] {
            bus.note_started();
            bus.finish(position);
        }
        assert_eq!(bus.next_ready(), None, "a1 is still running");
        bus.note_started();
        bus.finish(2);
        assert_eq!(bus.next_ready(), Some(1));
        assert_eq!(bus.next_ready(), None);
        bus.note_started();
        bus.finish(1);
        assert_eq!(bus.next_ready(), Some(0));
        assert!(!bus.complete(), "the root has not started");
        bus.note_started();
        bus.finish(0);
        assert_eq!(bus.next_ready(), None);
        assert!(bus.complete());
    }
}

#[test]
fn a_flat_plan_has_no_buses_and_deep_nesting_has_no_depth_limit() {
    let flat = Bus::new(vec![None; 4]);
    assert_eq!(flat.roots().collect::<Vec<_>>(), [0, 1, 2, 3]);
    assert!((0..4).all(|position| !flat.has_children(position)));
    // A chain deeper than any fixed stack: each track is the next one's parent.
    let depth: usize = 10_000;
    let parents = (0..depth).map(|index| index.checked_sub(1)).collect();
    let mut chain = Bus::new(parents);
    chain.begin();
    let mut position = depth - 1;
    loop {
        chain.note_started();
        chain.finish(position);
        match chain.next_ready() {
            Some(next) => position = next,
            None => break,
        }
    }
    assert_eq!(position, 0);
    assert!(chain.complete());
}
