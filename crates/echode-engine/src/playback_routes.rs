//! Audio edges prepared from the same routes that PDC solves. Edge planes keep
//! a pre-fader tap alive even after its folder/master consumes the source.
use super::*;

pub(super) struct Edge {
    id: u128,
    source: u32,
    target: (u32, Option<u128>),
    left: Vec<f32>,
    right: Vec<f32>,
    stereo: bool,
    delay: Delay,
    fader: FaderState,
}

pub(super) struct KeyInput {
    device: u128,
    edges: Vec<usize>,
    channels: usize,
}

#[derive(Default)]
pub(super) struct AudioRoutes {
    edges: Vec<Edge>,
    by_id: Vec<(u128, usize)>,
    outgoing: Vec<Vec<usize>>,
    sends: Vec<Vec<usize>>,
    keys: Vec<Vec<KeyInput>>,
}

impl AudioRoutes {
    pub(super) fn new(
        prepared: &Prepared,
        active_keys: &[PreparedSidechain],
        send_delays: &[u64],
        key_delays: &[u64],
        frames: usize,
    ) -> Result<Self, PlaybackError> {
        let count = prepared.tracks.len();
        let mut bank = Self {
            outgoing: vec![Vec::new(); count],
            sends: vec![Vec::new(); count],
            keys: (0..count).map(|_| Vec::new()).collect(),
            ..Self::default()
        };
        for ((route, &(id, gain)), &delay) in prepared
            .sends
            .iter()
            .zip(&prepared.send_settings)
            .zip(send_delays)
        {
            let edge = bank.push(
                prepared, id, route.from, route.to, None, gain, delay, frames,
            )?;
            bank.sends
                .get_mut(route.to)
                .ok_or(PlaybackError::Misaligned)?
                .push(edge);
        }
        for (route, &delay) in active_keys.iter().zip(key_delays) {
            let edge = bank.push(
                prepared,
                route.id,
                route.from,
                route.track,
                Some(route.device),
                route.gain,
                delay,
                frames,
            )?;
            let keys = bank
                .keys
                .get_mut(route.track)
                .ok_or(PlaybackError::Misaligned)?;
            if let Some(key) = keys.iter_mut().find(|key| key.device == route.device) {
                key.edges.push(edge);
            } else {
                keys.push(KeyInput {
                    device: route.device,
                    edges: vec![edge],
                    channels: 2,
                });
            }
        }
        bank.by_id = bank
            .edges
            .iter()
            .enumerate()
            .map(|(index, edge)| (edge.id, index))
            .collect();
        bank.by_id.sort_unstable_by_key(|entry| entry.0);
        Ok(bank)
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        prepared: &Prepared,
        id: u128,
        from: usize,
        to: usize,
        device: Option<u128>,
        gain: f32,
        delay: u64,
        frames: usize,
    ) -> Result<usize, PlaybackError> {
        let source = prepared.tracks.get(from).ok_or(PlaybackError::Misaligned)?;
        let target = prepared
            .tracks
            .get(to)
            .ok_or(PlaybackError::Misaligned)?
            .key;
        let index = self.edges.len();
        self.edges.push(Edge {
            id,
            source: source.key,
            target: (target, device),
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            stereo: false,
            delay: Delay::new(delay, frames)?,
            fader: FaderState::new(
                TrackFader {
                    gain,
                    pan: 0.0,
                    mute: source.base_fader.mute,
                    solo: false,
                },
                prepared.sample_rate,
            ),
        });
        self.outgoing
            .get_mut(from)
            .ok_or(PlaybackError::Misaligned)?
            .push(index);
        Ok(index)
    }

    pub(super) fn configure(
        &mut self,
        hosts: &HostGraph,
        tracks: &[Track],
    ) -> Result<(), PlaybackError> {
        for keys in &mut self.keys {
            for key in keys {
                key.channels = hosts.sidechain_channels(key.device)?;
                for &edge in &key.edges {
                    let edge = self.edges.get(edge).ok_or(PlaybackError::Misaligned)?;
                    let source = tracks
                        .iter()
                        .find(|track| track.key == edge.source)
                        .ok_or(PlaybackError::Misaligned)?;
                    if key.channels == 1 && hosts.output_stereo(source.key, source.stereo) {
                        return Err(PlaybackError::SidechainMapping { device: key.device });
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn capture(
        &mut self,
        position: usize,
        track: &Track,
        frames: usize,
    ) -> Result<(), PlaybackError> {
        for &index in self
            .outgoing
            .get(position)
            .ok_or(PlaybackError::Misaligned)?
        {
            let edge = self.edges.get_mut(index).ok_or(PlaybackError::Misaligned)?;
            edge.stereo = track.stereo;
            let left = edge
                .left
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let right = edge
                .right
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let source = track
                .plane
                .get(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let source_right = if track.stereo {
                track
                    .right
                    .get(..frames)
                    .ok_or(PlaybackError::BufferTooSmall)?
            } else {
                source
            };
            left.copy_from_slice(source);
            right.copy_from_slice(source_right);
            edge.delay.process(left, Some(right));
        }
        Ok(())
    }

    pub(super) fn has_sends(&self, position: usize) -> bool {
        self.sends
            .get(position)
            .is_some_and(|edges| !edges.is_empty())
    }

    pub(super) fn sum_sends(
        &mut self,
        position: usize,
        left: &mut [f64],
        right: &mut [f64],
    ) -> Result<(), PlaybackError> {
        for &index in self.sends.get(position).ok_or(PlaybackError::Misaligned)? {
            self.edges
                .get_mut(index)
                .ok_or(PlaybackError::Misaligned)?
                .add(left, right, false)?;
        }
        Ok(())
    }

    pub(super) fn supply_keys(
        &mut self,
        position: usize,
        hosts: &mut HostGraph,
        left: &mut [f64],
        right: &mut [f64],
    ) -> Result<(), PlaybackError> {
        for key in self.keys.get(position).ok_or(PlaybackError::Misaligned)? {
            left.fill(0.0);
            right.fill(0.0);
            for &index in &key.edges {
                let edge = self.edges.get_mut(index).ok_or(PlaybackError::Misaligned)?;
                // Mono keys have no promotion; stereo keys use the same equal
                // power promotion as a track input. Stereo -> mono was refused.
                edge.add(left, right, key.channels == 1)?;
            }
            hosts.supply_sidechain(key.device, left, right)?;
        }
        Ok(())
    }

    pub(super) fn carry(&mut self, previous: &mut Self, playing: bool) {
        for edge in &mut self.edges {
            let old = previous
                .by_id
                .binary_search_by_key(&edge.id, |entry| entry.0)
                .ok()
                .and_then(|index| previous.by_id.get(index))
                .map(|entry| entry.1);
            if let Some(old) = old
                .and_then(|index| previous.edges.get_mut(index))
                .filter(|old| old.source == edge.source && old.target == edge.target)
            {
                edge.delay.carry(&mut old.delay);
                if playing {
                    edge.fader.carry(old.fader);
                }
            }
        }
    }

    pub(super) fn clear(&mut self) {
        for edge in &mut self.edges {
            edge.delay.clear();
        }
    }
}

impl Edge {
    fn add(
        &mut self,
        left: &mut [f64],
        right: &mut [f64],
        mono_key: bool,
    ) -> Result<(), PlaybackError> {
        let signal = Signal {
            left: self
                .left
                .get(..left.len())
                .ok_or(PlaybackError::BufferTooSmall)?,
            right: if self.stereo || mono_key {
                Some(
                    self.right
                        .get(..right.len())
                        .ok_or(PlaybackError::BufferTooSmall)?,
                )
            } else {
                None
            },
        };
        self.fader.add(signal, left, right);
        Ok(())
    }
}
