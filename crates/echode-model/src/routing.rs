//! Aux sends and plugin sidechain routes across tracks.
//!
//! Sends leave their source track post-FX, pre-fader and enter their target
//! track's bus input. Sidechains leave their source track at the same tap and
//! enter a specific CLAP device on the receiving track. Together with folder
//! parent edges (`child -> parent`), they must form a directed acyclic graph.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::device::{DeviceKind, GAIN_MAX};
use crate::error::InvariantError;
use crate::id::Id;
use crate::model::{ModelError, Project, Track};
use crate::route_fields::{send_fields, sidechain_fields};

/// Unity linear gain (`0 dB`) used when a route is created without a level.
pub const ROUTE_GAIN_DEFAULT: f32 = 1.0;

/// Finite non-negative linear route gain in `0.0..=GAIN_MAX`, rejecting `-0.0`
/// so bitwise snapshot comparisons have one canonical zero.
pub fn route_gain_allowed(gain: f32) -> bool {
    gain.is_finite() && !gain.is_sign_negative() && gain <= GAIN_MAX
}

pub(crate) fn canonical_route_gain(gain: f32) -> Result<f32, ModelError> {
    if !gain.is_finite() || !(0.0..=GAIN_MAX).contains(&gain) {
        return Err(ModelError::InvalidGain);
    }
    Ok(if gain == 0.0 { 0.0 } else { gain })
}

/// Signal tap on the source track where a send or sidechain reads audio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum RouteTap {
    #[default]
    #[serde(rename = "post_fx_pre_fader")]
    PostFxPreFader,
}

impl RouteTap {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PostFxPreFader => "post_fx_pre_fader",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "post_fx_pre_fader" => Some(Self::PostFxPreFader),
            _ => None,
        }
    }
}

macro_rules! define_send_route {
    ($($property:ident => $field:ident: $type:ty, $value:ident, $emit:ident, $parse:ident; skip $skip:ident;)+) => {
        /// A post-FX, pre-fader auxiliary send from the owning track to `target`.
        #[derive(Clone, Debug, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct SendRoute {
            pub id: Id,
            $(pub $field: $type,)+
        }
    };
}

send_fields!(define_send_route);

impl SendRoute {
    pub fn new(id: Id, target: Id, gain: f32) -> Result<Self, ModelError> {
        Ok(Self {
            id,
            target,
            tap: RouteTap::PostFxPreFader,
            gain: canonical_route_gain(gain)?,
        })
    }
}

impl PartialEq for SendRoute {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.target == other.target
            && self.tap == other.tap
            && self.gain.to_bits() == other.gain.to_bits()
    }
}

impl Eq for SendRoute {}

macro_rules! define_sidechain_route {
    ($($property:ident => $field:ident: $type:ty, $value:ident, $emit:ident, $parse:ident; skip $skip:ident;)+) => {
        /// An auxiliary audio feed from `source` into the owning CLAP device.
        #[derive(Clone, Debug, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct SidechainRoute {
            pub id: Id,
            $(pub $field: $type,)+
        }
    };
}

sidechain_fields!(define_sidechain_route);

impl SidechainRoute {
    pub fn new(id: Id, source: Id, gain: f32) -> Result<Self, ModelError> {
        Ok(Self {
            id,
            source,
            tap: RouteTap::PostFxPreFader,
            gain: canonical_route_gain(gain)?,
        })
    }
}

impl PartialEq for SidechainRoute {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.source == other.source
            && self.tap == other.tap
            && self.gain.to_bits() == other.gain.to_bits()
    }
}

impl Eq for SidechainRoute {}

pub(crate) fn validate_sends(
    sends: &[SendRoute],
    seen: &mut HashSet<Id>,
) -> Result<(), ModelError> {
    for send in sends {
        if !seen.insert(send.id) {
            return Err(ModelError::DuplicateId);
        }
        if !route_gain_allowed(send.gain) {
            return Err(ModelError::InvalidGain);
        }
    }
    Ok(())
}

pub(crate) fn validate_sidechains(
    sidechains: &[SidechainRoute],
    seen: &mut HashSet<Id>,
) -> Result<(), ModelError> {
    for sidechain in sidechains {
        if !seen.insert(sidechain.id) {
            return Err(ModelError::DuplicateId);
        }
        if !route_gain_allowed(sidechain.gain) {
            return Err(ModelError::InvalidGain);
        }
    }
    Ok(())
}

pub(crate) fn validate_routes(tracks: &[Arc<Track>]) -> Result<(), ModelError> {
    let index_by_id: HashMap<Id, usize> = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.id, index))
        .collect();
    let mut targets = HashSet::new();
    let mut sources = HashSet::new();
    for track in tracks {
        targets.clear();
        for send in &track.sends {
            if !index_by_id.contains_key(&send.target) {
                return Err(ModelError::MissingTrack);
            }
            if !targets.insert(send.target) {
                return Err(ModelError::DuplicateRoute);
            }
        }
        for device in &track.devices {
            sources.clear();
            for sidechain in &device.sidechains {
                if !index_by_id.contains_key(&sidechain.source) {
                    return Err(ModelError::MissingTrack);
                }
                if !sources.insert(sidechain.source) {
                    return Err(ModelError::DuplicateRoute);
                }
            }
        }
    }
    check_routing_acyclic_with_index(tracks, &index_by_id).map_err(|_| ModelError::Cycle)
}

pub(crate) fn check_route_invariants(tracks: &[Arc<Track>]) -> Result<(), InvariantError> {
    let index_by_id: HashMap<Id, usize> = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.id, index))
        .collect();
    let mut targets = HashSet::new();
    let mut sources = HashSet::new();
    for track in tracks {
        targets.clear();
        for send in &track.sends {
            if !route_gain_allowed(send.gain) {
                return Err(InvariantError::RouteGain { route: send.id });
            }
            if !index_by_id.contains_key(&send.target) {
                return Err(InvariantError::MissingRouteTarget {
                    route: send.id,
                    track: send.target,
                });
            }
            if !targets.insert(send.target) {
                return Err(InvariantError::DuplicateRoute { route: send.id });
            }
        }
        for device in &track.devices {
            if device.kind == DeviceKind::Mixer && !device.sidechains.is_empty() {
                return Err(InvariantError::InvalidMixer {
                    track: track.id,
                    name: track.name.clone(),
                });
            }
            sources.clear();
            for sidechain in &device.sidechains {
                if !route_gain_allowed(sidechain.gain) {
                    return Err(InvariantError::RouteGain {
                        route: sidechain.id,
                    });
                }
                if !index_by_id.contains_key(&sidechain.source) {
                    return Err(InvariantError::MissingRouteSource {
                        route: sidechain.id,
                        track: sidechain.source,
                    });
                }
                if !sources.insert(sidechain.source) {
                    return Err(InvariantError::DuplicateRoute {
                        route: sidechain.id,
                    });
                }
            }
        }
    }
    check_routing_acyclic_with_index(tracks, &index_by_id)
        .map_err(|track| InvariantError::RouteCycle { track })
}

/// Checks that folder buses (`child -> parent`), sends (`source -> target`),
/// and sidechains (`source -> device.track`) form a DAG, returning the id of a
/// track on the first cycle encountered.
pub(crate) fn check_routing_acyclic(tracks: &[Arc<Track>]) -> Result<(), Id> {
    let index_by_id: HashMap<Id, usize> = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.id, index))
        .collect();
    check_routing_acyclic_with_index(tracks, &index_by_id)
}

fn check_routing_acyclic_with_index(
    tracks: &[Arc<Track>],
    index_by_id: &HashMap<Id, usize>,
) -> Result<(), Id> {
    let count = tracks.len();
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (index, track) in tracks.iter().enumerate() {
        if let Some(parent) = track.parent
            && let Some(&parent_index) = index_by_id.get(&parent)
        {
            outgoing[index].push(parent_index);
        }
        for send in &track.sends {
            if let Some(&target_index) = index_by_id.get(&send.target) {
                outgoing[index].push(target_index);
            }
        }
        for device in &track.devices {
            for sidechain in &device.sidechains {
                if let Some(&source_index) = index_by_id.get(&sidechain.source) {
                    outgoing[source_index].push(index);
                }
            }
        }
    }

    // 0 = unvisited, 1 = on the active DFS stack, 2 = verified acyclic.
    // Explicit stack frames keep arbitrary nesting and routing depth off the OS stack.
    let mut state = vec![0u8; count];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in 0..count {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        stack.push((start, 0));
        while let Some((node, cursor)) = stack.last_mut() {
            let current = *node;
            if let Some(&next) = outgoing[current].get(*cursor) {
                *cursor += 1;
                match state[next] {
                    1 => return Err(tracks[next].id),
                    0 => {
                        state[next] = 1;
                        stack.push((next, 0));
                    }
                    _ => {}
                }
            } else {
                state[current] = 2;
                stack.pop();
            }
        }
    }
    Ok(())
}

impl Project {
    /// Adds a post-FX, pre-fader send from `source` to `target` with a fresh id.
    pub fn add_send(&self, source: Id, target: Id, gain: f32) -> Result<(Self, Id), ModelError> {
        let id = Id::generate();
        let next = self.add_send_with_id(id, source, target, gain)?;
        Ok((next, id))
    }

    /// Adds a post-FX, pre-fader send with a caller-supplied `id`.
    pub fn add_send_with_id(
        &self,
        id: Id,
        source: Id,
        target: Id,
        gain: f32,
    ) -> Result<Self, ModelError> {
        let send = SendRoute::new(id, target, gain)?;
        if self.id_in_use(id) {
            return Err(ModelError::DuplicateId);
        }
        let source_index = self.track_index(source)?;
        let _ = self.track_index(target)?;
        if self.tracks()[source_index]
            .sends
            .iter()
            .any(|existing| existing.target == target)
        {
            return Err(ModelError::DuplicateRoute);
        }
        let mut track = (*self.tracks()[source_index]).clone();
        track.sends.push(send);
        let next = self.write_track(source_index, track);
        check_routing_acyclic(next.tracks()).map_err(|_| ModelError::Cycle)?;
        Ok(next)
    }

    /// Sets the linear gain (`0.0..=GAIN_MAX`) of an existing send.
    pub fn set_send_gain(&self, send_id: Id, gain: f32) -> Result<Self, ModelError> {
        let gain = canonical_route_gain(gain)?;
        let (track_index, send_index) =
            self.locate_send(send_id).ok_or(ModelError::MissingRoute)?;
        if self.tracks()[track_index].sends[send_index].gain.to_bits() == gain.to_bits() {
            return Ok(self.clone());
        }
        let mut track = (*self.tracks()[track_index]).clone();
        track.sends[send_index].gain = gain;
        Ok(self.write_track(track_index, track))
    }

    /// Removes an existing send by id.
    pub fn remove_send(&self, send_id: Id) -> Result<Self, ModelError> {
        let (track_index, send_index) =
            self.locate_send(send_id).ok_or(ModelError::MissingRoute)?;
        let mut track = (*self.tracks()[track_index]).clone();
        track.sends.remove(send_index);
        Ok(self.write_track(track_index, track))
    }

    /// Adds a post-FX, pre-fader sidechain route from `source` into `device`
    /// with a fresh id.
    pub fn add_sidechain(
        &self,
        source: Id,
        device: Id,
        gain: f32,
    ) -> Result<(Self, Id), ModelError> {
        let id = Id::generate();
        let next = self.add_sidechain_with_id(id, source, device, gain)?;
        Ok((next, id))
    }

    /// Adds a post-FX, pre-fader sidechain route with a caller-supplied `id`.
    pub fn add_sidechain_with_id(
        &self,
        id: Id,
        source: Id,
        device_id: Id,
        gain: f32,
    ) -> Result<Self, ModelError> {
        let sidechain = SidechainRoute::new(id, source, gain)?;
        if self.id_in_use(id) {
            return Err(ModelError::DuplicateId);
        }
        let _ = self.track_index(source)?;
        let (track_index, device_index) = self.locate_clap_for_route(device_id)?;
        if self.tracks()[track_index].devices[device_index]
            .sidechains
            .iter()
            .any(|existing| existing.source == source)
        {
            return Err(ModelError::DuplicateRoute);
        }
        let mut track = (*self.tracks()[track_index]).clone();
        track.devices[device_index].sidechains.push(sidechain);
        let next = self.write_track(track_index, track);
        check_routing_acyclic(next.tracks()).map_err(|_| ModelError::Cycle)?;
        Ok(next)
    }

    /// Sets the linear gain (`0.0..=GAIN_MAX`) of an existing sidechain route.
    pub fn set_sidechain_gain(&self, sidechain_id: Id, gain: f32) -> Result<Self, ModelError> {
        let gain = canonical_route_gain(gain)?;
        let (track_index, device_index, sidechain_index) = self
            .locate_sidechain(sidechain_id)
            .ok_or(ModelError::MissingRoute)?;
        if self.tracks()[track_index].devices[device_index].sidechains[sidechain_index]
            .gain
            .to_bits()
            == gain.to_bits()
        {
            return Ok(self.clone());
        }
        let mut track = (*self.tracks()[track_index]).clone();
        track.devices[device_index].sidechains[sidechain_index].gain = gain;
        Ok(self.write_track(track_index, track))
    }

    /// Removes an existing sidechain route by id.
    pub fn remove_sidechain(&self, sidechain_id: Id) -> Result<Self, ModelError> {
        let (track_index, device_index, sidechain_index) = self
            .locate_sidechain(sidechain_id)
            .ok_or(ModelError::MissingRoute)?;
        let mut track = (*self.tracks()[track_index]).clone();
        track.devices[device_index]
            .sidechains
            .remove(sidechain_index);
        Ok(self.write_track(track_index, track))
    }

    /// Returns `(source_track_id, send)` for `send_id`.
    pub fn send(&self, send_id: Id) -> Option<(Id, &SendRoute)> {
        let (track_index, send_index) = self.locate_send(send_id)?;
        let track = &self.tracks()[track_index];
        Some((track.id, &track.sends[send_index]))
    }

    /// Returns `(target_track_id, device_id, sidechain)` for `sidechain_id`.
    pub fn sidechain(&self, sidechain_id: Id) -> Option<(Id, Id, &SidechainRoute)> {
        let (track_index, device_index, sidechain_index) = self.locate_sidechain(sidechain_id)?;
        let track = &self.tracks()[track_index];
        let device = &track.devices[device_index];
        Some((track.id, device.id, &device.sidechains[sidechain_index]))
    }

    fn locate_send(&self, send_id: Id) -> Option<(usize, usize)> {
        for (track_index, track) in self.tracks().iter().enumerate() {
            if let Some(send_index) = track.sends.iter().position(|send| send.id == send_id) {
                return Some((track_index, send_index));
            }
        }
        None
    }

    fn locate_sidechain(&self, sidechain_id: Id) -> Option<(usize, usize, usize)> {
        for (track_index, track) in self.tracks().iter().enumerate() {
            for (device_index, device) in track.devices.iter().enumerate() {
                if let Some(sidechain_index) = device
                    .sidechains
                    .iter()
                    .position(|sidechain| sidechain.id == sidechain_id)
                {
                    return Some((track_index, device_index, sidechain_index));
                }
            }
        }
        None
    }

    fn locate_clap_for_route(&self, device_id: Id) -> Result<(usize, usize), ModelError> {
        for (track_index, track) in self.tracks().iter().enumerate() {
            if let Some(device_index) = track
                .devices
                .iter()
                .position(|device| device.id == device_id && device.kind == DeviceKind::Clap)
            {
                return Ok((track_index, device_index));
            }
        }
        Err(ModelError::InvalidDevice)
    }
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
