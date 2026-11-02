//! Plugin bytes captured in the background, waiting for their turn to be
//! recorded on the worker.

use std::collections::BTreeSet;

use echode_audio_io::PluginActivity;
use echode_model::Project;

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

    /// A newer save of a device replaces its older one, which nothing would
    /// record on its own: an edit's commit or the next background record
    /// takes the newest bytes.
    pub(crate) fn add(&mut self, saves: Vec<(u128, Vec<u8>)>) {
        for (device, bytes) in saves {
            self.pending.retain(|(known, _)| *known != device);
            self.pending.push((device, bytes));
        }
    }

    /// Devices whose helpers hold a saved change that an edit in progress
    /// will record. Their helpers must outlive other document changes.
    pub(crate) fn held(&self) -> Vec<u128> {
        self.pending
            .iter()
            .map(|(device, _)| *device)
            .filter(|device| self.editing.contains(device))
            .collect()
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

    /// Another edit, Undo or Redo changed the document from `before` to
    /// `after`. An edit in progress continues on a device that kept its plugin
    /// and recorded state, whose helper the session kept with its held save
    /// (ADR 0229). Any other device's edit ends, and its saves, captured for
    /// the previous graph, must not land on the new one.
    pub(crate) fn document_changed(&mut self, before: &Project, after: &Project) {
        let recorded = |project: &Project, device: u128| {
            project
                .tracks()
                .iter()
                .flat_map(|track| &track.devices)
                .find(|item| item.id.raw() == device)
                .map(|item| (item.plugin.clone(), item.state.clone(), item.offline))
        };
        self.editing.retain(|&device| {
            let kept = recorded(before, device);
            kept.is_some() && kept == recorded(after, device)
        });
        let editing = &self.editing;
        self.pending.retain(|(device, _)| editing.contains(device));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_audio_io::{ParamEvent, ParamEventKind};
    use echode_model::Id;

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

    /// Devices on one track, each with the state hash spelled by repeating its digit.
    fn with_devices(states: &[(u128, Option<char>)]) -> Project {
        let (mut project, track) = Project::new().add_track("fx");
        for &(device, digit) in states {
            let hash = digit.map(|digit| digit.to_string().repeat(64));
            project = project
                .add_clap_device(track, Id::from_raw(device), "dev.echode.test-params")
                .unwrap()
                .set_device_state(Id::from_raw(device), hash)
                .unwrap();
        }
        project
    }

    #[test]
    fn a_changed_device_drops_its_saves_and_ends_its_edit() {
        let before = with_devices(&[(FX, Some('a')), (SYNTH, None)]);
        let undone = with_devices(&[(FX, Some('b')), (SYNTH, None)]);
        let removed = with_devices(&[(SYNTH, None)]);
        for after in [undone, removed] {
            let mut saves = PluginSaves::default();
            saves.editing.insert(FX);
            saves.add(vec![(FX, b"old graph".to_vec()), (SYNTH, b"old".to_vec())]);
            saves.document_changed(&before, &after);
            assert!(saves.editing.is_empty());
            assert!(saves.held().is_empty());
            assert_eq!(offered(&mut saves, true), (Vec::new(), false));
        }
    }

    /// DOC-A2 at the worker: an unrelated edit keeps the edit in progress and
    /// its held save, and the session is told to keep that helper.
    #[test]
    fn an_unrelated_edit_keeps_a_held_save_and_its_edit() {
        let before = with_devices(&[(FX, Some('a')), (SYNTH, None)]);
        let (after, _) = before.add_track("unrelated edit");
        let mut saves = PluginSaves::default();
        saves.editing.insert(FX);
        saves.add(vec![(FX, b"first".to_vec()), (FX, b"held".to_vec())]);
        saves.add(vec![(SYNTH, b"refused".to_vec())]);
        assert_eq!(saves.held(), [FX]);
        saves.document_changed(&before, &after);
        assert_eq!(saves.held(), [FX], "the session would replace the helper");
        assert!(saves.editing.contains(&FX));
        // The edit's commit ends it; only then is the newest save offered.
        saves.editing.remove(&FX);
        assert_eq!(
            offered(&mut saves, true),
            (vec![(FX, b"held".to_vec())], true)
        );
    }
}
