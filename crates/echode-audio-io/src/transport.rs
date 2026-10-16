use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::AudioCommand;

/// Stop invalidates earlier starts before the document worker can dequeue it.
/// The callback only reads this generation; preparation and cleanup stay off RT.
#[derive(Clone, Default)]
pub struct TransportCancellation(Arc<AtomicU64>);

impl TransportCancellation {
    pub fn request(&self, command: &AudioCommand) -> u64 {
        if matches!(command, AudioCommand::Pause | AudioCommand::Stop) {
            self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
        } else {
            self.0.load(Ordering::Acquire)
        }
    }

    pub fn is_current(&self, revision: u64) -> bool {
        self.0.load(Ordering::Acquire) == revision
    }
}
