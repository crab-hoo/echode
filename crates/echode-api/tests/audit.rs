//! Open items from docs/reviews/2026-10-08-audit.md. Each test fails until its item
//! is fixed; `make known-bugs` runs them. Remove the `#[ignore]` with the fix.

use echode_api::bind_socket;

#[test]
#[ignore = "API-3: bind_socket deletes whatever is at the path"]
fn audit_api3_binding_over_a_regular_file_keeps_the_file() {
    let path = std::env::temp_dir().join(format!("echode-audit-api3-{}.json", std::process::id()));
    std::fs::write(&path, b"{}").unwrap();
    let bound = bind_socket(&path);
    let kept = std::fs::read(&path).ok();
    let _ = std::fs::remove_file(&path);
    assert!(bound.is_err(), "a regular file was replaced by a socket");
    assert_eq!(kept.as_deref(), Some(&b"{}"[..]));
}
