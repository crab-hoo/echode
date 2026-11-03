//! Flatten folder mute/solo gates on control, before the callback sees a bank.
use echode_model::{Id, Project};
use echode_realtime::Plan;

use crate::{PlaybackError, TrackFader};

pub(super) fn track_faders(
    project: &Project,
    plan: &Plan,
) -> Result<Vec<TrackFader>, PlaybackError> {
    if project.tracks().len() != plan.tracks.len() {
        return Err(PlaybackError::InvalidFader);
    }
    let any_solo = plan.tracks.iter().any(|track| track.solo != 0.0);
    let mut ancestors: Vec<(Id, bool, bool)> = Vec::new();
    let mut faders = Vec::with_capacity(plan.tracks.len());
    // The model's preorder puts each parent before its contiguous subtree.
    // Each entry is pushed/popped once: no recursion or per-track parent scan.
    for (track, planned) in project.tracks().iter().zip(&plan.tracks) {
        if track.id.raw() != planned.track {
            return Err(PlaybackError::InvalidFader);
        }
        while ancestors.last().map(|entry| entry.0) != track.parent {
            if ancestors.pop().is_none() {
                return Err(PlaybackError::InvalidFader);
            }
        }
        let (parent_mute, parent_solo) = ancestors
            .last()
            .map(|entry| (entry.1, entry.2))
            .unwrap_or_default();
        let mute = parent_mute || planned.mute != 0.0;
        let solo = parent_solo || planned.solo != 0.0;
        ancestors.push((track.id, mute, solo));
        faders.push(TrackFader {
            gain: planned.gain,
            pan: planned.pan,
            mute: mute || (any_solo && !solo),
            // The global solo gate is already folded into mute. Gain/pan remain
            // local until folder buses are implemented; no child audio is routed.
            solo: false,
        });
    }
    Ok(faders)
}
