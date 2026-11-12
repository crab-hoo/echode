//! The state bytes a helper last saved or loaded, kept for control threads.
//!
//! The playback engine decides whether a running helper already holds a
//! device's recorded state by comparing names, not bytes (ADR 0329). The name
//! is the object store's, computed by the caller's function so this crate
//! keeps no second copy of that rule. It is computed at most once for the
//! bytes, and every clone shares it.

use std::fmt;
use std::sync::{Arc, OnceLock};

/// Bytes a helper last saved or loaded, and their name once someone asks.
#[derive(Clone)]
pub struct AppliedState {
    bytes: Arc<[u8]>,
    name: Arc<OnceLock<String>>,
}

impl AppliedState {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            bytes: bytes.into(),
            name: Arc::new(OnceLock::new()),
        }
    }

    /// Bytes whose name the caller already has, such as a recorded object read
    /// by that name. The caller vouches that `name` is what the naming
    /// function would give these bytes.
    pub fn named(bytes: impl Into<Arc<[u8]>>, name: String) -> Self {
        Self {
            bytes: bytes.into(),
            name: Arc::new(OnceLock::from(name)),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The name `hash` gives these bytes. The first call computes it; later
    /// calls, on this value or any clone, return the same text.
    pub fn name(&self, hash: impl FnOnce(&[u8]) -> String) -> &str {
        self.name.get_or_init(|| hash(&self.bytes))
    }
}

impl fmt::Debug for AppliedState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppliedState")
            .field("len", &self.bytes.len())
            .field("name", &self.name.get())
            .finish()
    }
}

/// Equal bytes, whatever is known of their names.
impl PartialEq for AppliedState {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for AppliedState {}

#[cfg(test)]
#[path = "applied_tests.rs"]
mod tests;
