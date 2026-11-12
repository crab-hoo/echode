use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn a_name_is_computed_once_and_shared_by_clones() {
    let calls = AtomicUsize::new(0);
    let hash = |bytes: &[u8]| {
        calls.fetch_add(1, Ordering::SeqCst);
        format!("{}", bytes.len())
    };
    let state = AppliedState::new(vec![1, 2, 3]);
    let clone = state.clone();
    assert_eq!(state.name(hash), "3");
    assert_eq!(clone.name(hash), "3");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_known_name_is_not_computed_again() {
    let state = AppliedState::named(vec![9], "recorded".into());
    assert_eq!(
        state.name(|_| unreachable!("the name was given")),
        "recorded"
    );
    assert_eq!(state.bytes(), &[9]);
}

#[test]
fn equality_is_the_bytes() {
    assert_eq!(
        AppliedState::named(vec![1], "a".into()),
        AppliedState::new(vec![1])
    );
    assert_ne!(AppliedState::new(vec![1]), AppliedState::new(vec![2]));
}
