//! The CLAP state a device's helper should hold, named as the object store
//! names it (ADR 0329).
//!
//! Two states with one name are the same bytes. A replacement compares the
//! name a route asks for with the name of what the running helper holds, and
//! reads a recorded state only for a helper that must load it: a new helper,
//! or one whose state differs. An edit that keeps every helper reads nothing,
//! however large the saved states are. With background preparation that read
//! happens on the preparation thread, not on the document's control turn.

use std::fmt;
use std::sync::Arc;

use echode_sandbox::AppliedState;

/// Why a recorded state could not be read. The reader's error is the cause.
pub type StateReadError = Arc<dyn std::error::Error + Send + Sync>;

/// Reads the recorded state its argument names. The bytes it returns must
/// have that name; the object store checks this when it reads.
pub type StateReader = Arc<dyn Fn(&str) -> Result<Vec<u8>, StateReadError> + Send + Sync>;

/// A device's state by name, with its bytes in memory or still in the store.
#[derive(Clone)]
pub struct PluginState {
    name: String,
    source: Source,
}

#[derive(Clone)]
enum Source {
    Loaded(AppliedState),
    Stored(StateReader),
}

impl PluginState {
    /// Bytes already in memory, named here.
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        let bytes = bytes.into();
        let name = echode_model::content_hash(&bytes);
        Self {
            source: Source::Loaded(AppliedState::named(bytes, name.clone())),
            name,
        }
    }

    /// What a running helper holds and the document has not recorded yet
    /// (ADR 0229). Its bytes are shared, not copied.
    pub fn held(applied: &AppliedState) -> Self {
        Self {
            name: applied_name(applied).to_owned(),
            source: Source::Loaded(applied.clone()),
        }
    }

    /// A recorded object, which `reader` reads only if a helper must load it.
    pub fn stored(name: String, reader: StateReader) -> Self {
        Self {
            name,
            source: Source::Stored(reader),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The bytes, reading a recorded object now. Each call reads again.
    pub fn load(&self) -> Result<AppliedState, StateReadError> {
        match &self.source {
            Source::Loaded(applied) => Ok(applied.clone()),
            Source::Stored(reader) => {
                Ok(AppliedState::named(reader(&self.name)?, self.name.clone()))
            }
        }
    }
}

/// Equal names are equal bytes, wherever the bytes are.
impl PartialEq for PluginState {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for PluginState {}

impl fmt::Debug for PluginState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = match self.source {
            Source::Loaded(_) => "loaded",
            Source::Stored(_) => "stored",
        };
        formatter
            .debug_struct("PluginState")
            .field("name", &self.name)
            .field("source", &source)
            .finish()
    }
}

/// The object store's name for what a helper holds, computed once per state.
pub(crate) fn applied_name(applied: &AppliedState) -> &str {
    applied.name(echode_model::content_hash)
}

/// Whether a helper holding `applied` already holds `wanted`. `None` on both
/// sides is the plugin's default.
pub(crate) fn holds(applied: Option<&AppliedState>, wanted: Option<&PluginState>) -> bool {
    match (applied, wanted) {
        (None, None) => true,
        (Some(applied), Some(wanted)) => applied_name(applied) == wanted.name(),
        _ => false,
    }
}

#[cfg(test)]
#[path = "plugin_state_tests.rs"]
mod tests;
