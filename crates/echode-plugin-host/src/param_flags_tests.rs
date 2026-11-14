use super::*;

#[test]
fn repeated_reads_share_metadata_until_a_rescan() {
    let cache = ParamFlags::new();
    let first = cache.read(|| HashMap::from([(1, 2)]));
    let second = cache.read(|| panic!("a warm cache must not enumerate the plugin"));
    assert!(Rc::ptr_eq(&first, &second));
    cache.invalidate();
    let next = cache.read(|| HashMap::from([(1, 4), (2, 8)]));
    assert_eq!(*next, HashMap::from([(1, 4), (2, 8)]));
    // A reader's owned snapshot stays valid after invalidation.
    assert_eq!(*first, HashMap::from([(1, 2)]));
}

#[test]
fn a_reentrant_rescan_does_not_panic_or_cache_the_invalidated_read() {
    let cache = ParamFlags::new();
    let first = cache.read(|| {
        cache.invalidate();
        HashMap::from([(1, 2)])
    });
    assert_eq!(*first, HashMap::from([(1, 2)]));
    let next = cache.read(|| HashMap::from([(1, 4)]));
    assert_eq!(*next, HashMap::from([(1, 4)]));
    let warm = cache.read(|| panic!("the fresh result must be cached"));
    assert!(Rc::ptr_eq(&next, &warm));
}
