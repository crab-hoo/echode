//! Plugin bytes captured in the background, waiting for their turn to be
//! recorded on the worker.

use std::collections::BTreeSet;

use echode_audio_io::PluginActivity;

#[derive(Default)]
pub(crate) struct PluginSaves {
    pending: Vec<(u128, Vec<u8>)>,
    /// Devices with values changed since their last commit, by the device view
    /// or by the plugin's own window. The commit that ends the edit supersedes
    /// their saves.
    pub(crate) editing: BTreeSet<u128>,
}

impl PluginSaves {
    /// Call before this turn's saves: a change the app will record as one Undo
    /// step must not be journaled first as a background save.
    pub(crate) fn note_activity(&mut self, device: u128, activity: &PluginActivity) {
        if crate::device::defers_saves(activity) {
            self.editing.insert(device);
        }
    }

    pub(crate) fn add(&mut self, saves: Vec<(u128, Vec<u8>)>) {
        self.pending.extend(saves);
    }

    /// Offers each save no edit is waiting on to `commit`, keeping those it
    /// refuses for a later turn. True when any was recorded.
    pub(crate) fn record(&mut self, mut commit: impl FnMut(u128, &[u8]) -> bool) -> bool {
        let editing = &self.editing;
        let mut recorded = false;
        self.pending.retain(|(device, bytes)| {
            if editing.contains(device) {
                return true;
            }
            let done = commit(*device, bytes);
            recorded |= done;
            !done
        });
        recorded
    }

    /// A save captured for the previous graph must not land on the new one. A
    /// commit, Undo or other edit also ends any parameter edit.
    pub(crate) fn document_changed(&mut self) {
        self.pending.clear();
        self.editing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_audio_io::{ParamEvent, ParamEventKind};

    const FX: u128 = 0xF1;
    const SYNTH: u128 = 0xF2;

    fn offered(saves: &mut PluginSaves, accept: bool) -> (Vec<(u128, Vec<u8>)>, bool) {
        let mut seen = Vec::new();
        let recorded = saves.record(|device, bytes| {
            seen.push((device, bytes.to_vec()));
            accept
        });
        (seen, recorded)
    }

    #[test]
    fn a_save_waits_for_the_edit_of_its_own_device_only() {
        let mut saves = PluginSaves::default();
        let mut gesture = PluginActivity::default();
        gesture.relay.events.push(ParamEvent {
            id: 1,
            kind: ParamEventKind::Begin,
        });
        saves.note_activity(FX, &gesture);
        saves.add(vec![(FX, b"edited".to_vec()), (SYNTH, b"other".to_vec())]);
        let (seen, recorded) = offered(&mut saves, true);
        assert_eq!(seen, [(SYNTH, b"other".to_vec())]);
        assert!(recorded);
        // The edit ended without a commit that records these bytes.
        saves.editing.remove(&FX);
        let (seen, recorded) = offered(&mut saves, true);
        assert_eq!(seen, [(FX, b"edited".to_vec())]);
        assert!(recorded);
        assert_eq!(offered(&mut saves, true), (Vec::new(), false));
    }

    #[test]
    fn a_refused_save_is_offered_again_in_a_later_turn() {
        let mut saves = PluginSaves::default();
        saves.add(vec![(FX, b"bytes".to_vec())]);
        let (seen, recorded) = offered(&mut saves, false);
        assert_eq!(seen.len(), 1);
        assert!(!recorded, "a refused save changed nothing");
        let (seen, recorded) = offered(&mut saves, true);
        assert_eq!(seen.len(), 1);
        assert!(recorded);
    }

    #[test]
    fn a_changed_document_drops_saves_and_ends_edits() {
        let mut saves = PluginSaves::default();
        saves.editing.insert(FX);
        saves.add(vec![(FX, b"old graph".to_vec()), (SYNTH, b"old".to_vec())]);
        saves.document_changed();
        assert!(saves.editing.is_empty());
        assert_eq!(offered(&mut saves, true), (Vec::new(), false));
    }
}
