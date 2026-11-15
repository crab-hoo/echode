//! Plugin captures waiting for a journal turn, including failed recordings.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use echode_audio_io::PluginActivity;
use echode_history::retry::BackgroundRetry;
use echode_history::{HostError, PluginState};
use echode_model::Project;

struct Pending {
    device: u128,
    state: Arc<PluginState>,
    retry: BackgroundRetry,
}

#[derive(Default)]
pub(crate) struct PluginSaves {
    pending: Vec<Pending>,
    /// Devices whose in-progress edit must be committed before background saves.
    pub(crate) editing: BTreeSet<u128>,
}

impl PluginSaves {
    pub(crate) fn note_activity(&mut self, device: u128, activity: &PluginActivity) {
        if crate::device::defers_saves(activity) {
            self.editing.insert(device);
        }
    }

    /// New bytes supersede the capture, but do not bypass its failure delay.
    pub(crate) fn add(&mut self, saves: Vec<(u128, Vec<u8>)>) {
        for (device, bytes) in saves {
            let state = Arc::new(PluginState::new(bytes));
            if let Some(pending) = self.pending.iter_mut().find(|save| save.device == device) {
                pending.state = state;
            } else {
                self.pending.push(Pending {
                    device,
                    state,
                    retry: Default::default(),
                });
            }
        }
    }

    /// Every unrecorded capture must keep its live helper across unrelated edits.
    pub(crate) fn held(&self) -> Vec<u128> {
        self.pending.iter().map(|save| save.device).collect()
    }

    pub(crate) fn record(
        &mut self,
        commit: impl FnMut(u128, Arc<PluginState>) -> Result<bool, HostError>,
    ) -> bool {
        self.record_with_clock(Instant::now, commit)
    }

    #[cfg(test)]
    fn record_at(
        &mut self,
        now: Instant,
        commit: impl FnMut(u128, Arc<PluginState>) -> Result<bool, HostError>,
    ) -> bool {
        self.record_with_clock(|| now, commit)
    }

    fn record_with_clock(
        &mut self,
        clock: impl Fn() -> Instant,
        mut commit: impl FnMut(u128, Arc<PluginState>) -> Result<bool, HostError>,
    ) -> bool {
        let editing = &self.editing;
        let mut recorded = false;
        let mut halted = false;
        self.pending.retain_mut(|save| {
            if halted || editing.contains(&save.device) || !save.retry.ready(clock()) {
                return true;
            }
            match commit(save.device, Arc::clone(&save.state)) {
                Ok(_) => {
                    recorded = true;
                    false
                }
                Err(error) => {
                    halted = error.durability_unknown();
                    if save.retry.failed(clock(), &error) && !halted {
                        log::warn!(
                            target: "echode_document::saves",
                            device = save.device.to_string().as_str(),
                            error:% = error,
                            cause:? = error;
                            "a captured plugin state could not be recorded; retrying later"
                        );
                    }
                    true
                }
            }
        });
        recorded
    }

    /// Keep a waiting capture across unrelated edits, but never apply it after
    /// its own plugin, recorded state, or offline status changes (ADR 0229).
    pub(crate) fn document_changed(&mut self, before: &Project, after: &Project) {
        let recorded = |project: &Project, device: u128| {
            project
                .tracks()
                .iter()
                .flat_map(|track| &track.devices)
                .find(|item| item.id.raw() == device)
                .map(|item| (item.plugin.clone(), item.state.clone(), item.offline))
        };
        let kept = |device| {
            let old = recorded(before, device);
            old.is_some() && old == recorded(after, device)
        };
        self.editing.retain(|&device| kept(device));
        self.pending.retain(|save| kept(save.device));
    }
}

#[cfg(test)]
#[path = "saves_tests.rs"]
mod tests;
