use super::*;
use std::sync::atomic::Ordering;

#[test]
fn a_capture_checks_its_bytes_once_but_recreates_a_missing_object() {
    let directory =
        std::env::temp_dir().join(format!("echode-capture-{}", echode_model::Id::generate()));
    let state = PluginState::new(b"captured bytes".to_vec());
    let hash = state.store(&directory).unwrap();
    for _ in 0..100 {
        assert_eq!(state.store(&directory).unwrap(), hash);
    }
    assert_eq!(state.full_checks.load(Ordering::Relaxed), 1);
    fs::remove_file(directory.join("media/objects").join(&hash)).unwrap();
    assert_eq!(state.store(&directory).unwrap(), hash);
    assert_eq!(read_object(&directory, &hash).unwrap(), b"captured bytes");
    assert_eq!(state.full_checks.load(Ordering::Relaxed), 2);
    let other = directory.join("other");
    assert_eq!(state.store(&other).unwrap(), hash);
    assert_eq!(read_object(&other, &hash).unwrap(), b"captured bytes");
    assert_eq!(state.full_checks.load(Ordering::Relaxed), 3);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn a_cached_capture_still_refuses_a_replaced_store_directory_link() {
    let directory = std::env::temp_dir().join(format!(
        "echode-capture-link-{}",
        echode_model::Id::generate()
    ));
    let state = PluginState::new(b"captured bytes".to_vec());
    state.store(&directory).unwrap();
    let objects = directory.join("media/objects");
    let moved = directory.join("moved-objects");
    fs::rename(&objects, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &objects).unwrap();
    assert!(
        state.store(&directory).is_err(),
        "the cached name bypassed the store boundary"
    );
    assert_eq!(state.full_checks.load(Ordering::Relaxed), 1);
    fs::remove_file(&objects).unwrap();
    fs::remove_dir_all(&moved).unwrap();
    let hash = state.store(&directory).unwrap();
    assert_eq!(read_object(&directory, &hash).unwrap(), b"captured bytes");
    let _ = fs::remove_dir_all(directory);
}
