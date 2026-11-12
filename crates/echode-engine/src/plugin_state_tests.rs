use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn counting_reader(bytes: Vec<u8>, reads: Arc<AtomicUsize>) -> StateReader {
    Arc::new(move |_| {
        reads.fetch_add(1, Ordering::SeqCst);
        Ok(bytes.clone())
    })
}

#[test]
fn a_stored_state_is_read_only_when_loaded() {
    let reads = Arc::new(AtomicUsize::new(0));
    let name = echode_model::content_hash(b"recorded");
    let state = PluginState::stored(
        name.clone(),
        counting_reader(b"recorded".to_vec(), reads.clone()),
    );
    let applied = AppliedState::new(b"recorded".to_vec());
    assert!(holds(Some(&applied), Some(&state)));
    assert_eq!(state, PluginState::from_bytes(b"recorded".to_vec()));
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "comparing names read the store"
    );

    let loaded = state.load().unwrap();
    assert_eq!(loaded.bytes(), b"recorded");
    // The name comes with the bytes; nothing hashes them again.
    assert_eq!(loaded.name(|_| unreachable!("named by the store")), name);
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[test]
fn a_helper_holds_only_the_named_state_or_the_default() {
    let applied = AppliedState::new(vec![1, 2, 3]);
    let same = PluginState::from_bytes(vec![1, 2, 3]);
    let other = PluginState::from_bytes(vec![4]);
    assert!(holds(None, None));
    assert!(holds(Some(&applied), Some(&same)));
    assert!(!holds(Some(&applied), Some(&other)));
    assert!(!holds(Some(&applied), None));
    assert!(!holds(None, Some(&same)));
}

#[test]
fn a_held_state_shares_the_helpers_bytes_and_name() {
    let applied = AppliedState::new(vec![7; 16]);
    let held = PluginState::held(&applied);
    assert_eq!(held.name(), echode_model::content_hash(&[7; 16]));
    assert_eq!(held.load().unwrap(), applied);
}

#[test]
fn a_failed_read_keeps_the_readers_cause() {
    let state = PluginState::stored(
        echode_model::content_hash(b"gone"),
        Arc::new(|_| {
            Err(Arc::new(std::io::Error::from(std::io::ErrorKind::NotFound)) as StateReadError)
        }),
    );
    let error = state.load().unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::NotFound)
    );
}
