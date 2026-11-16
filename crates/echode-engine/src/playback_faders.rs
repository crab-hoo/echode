//! Resolve folder mute/solo gates on control, before the callback sees a bank.
//!
//! A child reaches master through its parent's bus (ADR 0277, ADR 0338).
//! ADR 0244 still decides what is heard: a mute silences the track and every
//! descendant, a solo opens the track and every descendant. Each fader closes
//! when its own path is silent, so moving a track between folders never lets
//! a fader that a silent ancestor used to cover play at once.
use std::collections::HashMap;

use echode_model::{Id, Project};
use echode_realtime::Plan;

use crate::compensation::SendFlow;
use crate::{PlaybackError, TrackFader};

/// One track's gates.
pub(super) struct TrackGate {
    /// Closed (`mute`) when nothing this track outputs reaches its parent.
    pub(super) fader: TrackFader,
    /// Whether the track's own placed audio and first instrument are heard.
    /// Only a track that passes a soloed descendant without being soloed
    /// itself closes this while its fader stays open.
    pub(super) own: bool,
}

/// A model sidechain resolved to track positions, matched to its hosted device
/// stage when delay compensation runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PreparedSidechain {
    pub(super) from: usize,
    pub(super) track: usize,
    pub(super) device: u128,
    pub(super) id: u128,
    pub(super) gain: f32,
}

/// Gates, folder parents, and model routes in plan order.
pub(super) struct Routing {
    pub(super) gates: Vec<TrackGate>,
    pub(super) parents: Vec<Option<usize>>,
    pub(super) sends: Vec<SendFlow>,
    pub(super) send_settings: Vec<(u128, f32)>,
    pub(super) sidechains: Vec<PreparedSidechain>,
}

struct Ancestor {
    id: Id,
    position: usize,
    mute: bool,
    solo: bool,
}

pub(super) fn track_routing(project: &Project, plan: &Plan) -> Result<Routing, PlaybackError> {
    if project.tracks().len() != plan.tracks.len() {
        return Err(PlaybackError::InvalidFader);
    }
    let count = plan.tracks.len();
    let any_solo = plan.tracks.iter().any(|track| track.solo != 0.0);
    let mut ancestors: Vec<Ancestor> = Vec::new();
    let mut parents = Vec::with_capacity(count);
    let mut positions = HashMap::with_capacity(count);
    // (muted by itself or an ancestor, soloed by itself or an ancestor)
    let mut inherited = Vec::with_capacity(count);
    // The model's preorder puts each parent before its contiguous subtree.
    // Each entry is pushed/popped once: no recursion or per-track parent scan.
    for (position, (track, planned)) in project.tracks().iter().zip(&plan.tracks).enumerate() {
        if track.id.raw() != planned.track {
            return Err(PlaybackError::InvalidFader);
        }
        while ancestors.last().map(|entry| entry.id) != track.parent {
            if ancestors.pop().is_none() {
                return Err(PlaybackError::InvalidFader);
            }
        }
        let parent = ancestors.last();
        let mute = parent.is_some_and(|parent| parent.mute) || planned.mute != 0.0;
        let solo = parent.is_some_and(|parent| parent.solo) || planned.solo != 0.0;
        parents.push(parent.map(|parent| parent.position));
        positions.insert(track.id, position);
        inherited.push((mute, solo));
        ancestors.push(Ancestor {
            id: track.id,
            position,
            mute,
            solo,
        });
    }
    // A soloed track that no mute silences opens the bus of every ancestor.
    // A parent comes before its children, so one backward pass carries each
    // opening up to the top.
    let mut below = vec![false; count];
    for position in (0..count).rev() {
        let soloed = plan
            .tracks
            .get(position)
            .is_some_and(|track| track.solo != 0.0);
        let muted = inherited.get(position).is_some_and(|entry| entry.0);
        let opens = below.get(position).copied().unwrap_or(false) || (soloed && !muted);
        if opens
            && let Some(Some(parent)) = parents.get(position)
            && let Some(flag) = below.get_mut(*parent)
        {
            *flag = true;
        }
    }
    let gates = plan
        .tracks
        .iter()
        .zip(&inherited)
        .zip(&below)
        .map(|((planned, &(mute, solo)), &below)| {
            let heard = !mute && (!any_solo || solo);
            let passes = !mute && (!any_solo || solo || below);
            TrackGate {
                fader: TrackFader {
                    gain: planned.gain,
                    pan: planned.pan,
                    mute: !passes,
                    // The global solo gate is already folded into mute.
                    solo: false,
                },
                own: heard || !passes,
            }
        })
        .collect();
    let mut sends = Vec::new();
    let mut send_settings = Vec::new();
    let mut sidechains = Vec::new();
    for (position, track) in project.tracks().iter().enumerate() {
        for send in &track.sends {
            let &to = positions
                .get(&send.target)
                .ok_or(PlaybackError::InvalidFader)?;
            sends.push(SendFlow { from: position, to });
            send_settings.push((send.id.raw(), send.gain));
        }
        for device in &track.devices {
            if device.offline {
                continue;
            }
            for sidechain in &device.sidechains {
                let &from = positions
                    .get(&sidechain.source)
                    .ok_or(PlaybackError::InvalidFader)?;
                sidechains.push(PreparedSidechain {
                    from,
                    track: position,
                    device: device.id.raw(),
                    id: sidechain.id.raw(),
                    gain: sidechain.gain,
                });
            }
        }
    }
    Ok(Routing {
        gates,
        parents,
        sends,
        send_settings,
        sidechains,
    })
}
