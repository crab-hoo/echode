//! Plugin delay compensation, planned on control (ADR 0339, ADR 0340).
//!
//! The signal flow is a graph. A node is a point where its inputs meet,
//! followed by processing that takes `latency` frames. A link carries one
//! node's output to another node's input. A node without inputs reads the
//! timeline.
//!
//! [`Graph::solve`] gives every link the delay that makes all inputs of its
//! target meet at the same time, which is the latest of them. Every path from
//! the timeline to a node then takes the same number of frames: the node's
//! lag. The sink's lag after its own latency is how far the output is behind
//! the timeline, which an export removes.
//!
//! The solver knows nothing about tracks, folders, bypasses or sends. The
//! engine describes what it plays as nodes and links (`playback.rs`), and a
//! new merge point is a new link. [`Solution::verify`] checks the result
//! against the graph with rules that do not depend on the order the solver
//! worked in, and no plan is published without passing it.

use crate::PlaybackError;

/// A point where audio meets and is then processed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NodeId(usize);

/// One node's output on its way into another node, and the delay line there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LinkId(usize);

#[derive(Clone, Copy, Debug)]
struct Node {
    /// Frames between the node's input and its output.
    latency: u64,
    /// Declared tail of the node's processing. `u32::MAX` is infinite.
    tail: u32,
}

#[derive(Clone, Copy, Debug)]
struct Link {
    from: usize,
    to: usize,
}

/// What plays, as far as time is concerned. Built and solved on control.
#[derive(Debug, Default)]
pub(crate) struct Graph {
    nodes: Vec<Node>,
    links: Vec<Link>,
}

/// The delays that align a [`Graph`], in device frames.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Solution {
    /// Per link: frames to hold the source's output before the target.
    delays: Vec<u64>,
    /// Per node: how far its input is behind the timeline.
    lags: Vec<u64>,
    /// How far the sink's output is behind the timeline.
    latency: u64,
    /// The longest declared tail along any path into the sink.
    tail: u32,
}

impl Graph {
    pub(crate) fn node(&mut self, latency: u64, tail: u32) -> NodeId {
        self.nodes.push(Node { latency, tail });
        NodeId(self.nodes.len() - 1)
    }

    /// `from`'s output joins `to`'s input. Links between the same two nodes
    /// are separate paths with separate lines.
    pub(crate) fn link(&mut self, from: NodeId, to: NodeId) -> LinkId {
        self.links.push(Link {
            from: from.0,
            to: to.0,
        });
        LinkId(self.links.len() - 1)
    }

    /// Aligns every merge to its latest input, in time proportional to the
    /// nodes and links and without recursion, so neither the depth nor the
    /// width of the routing is limited.
    ///
    /// A loop has no finite alignment and is refused as
    /// [`PlaybackError::Feedback`]. A sum that does not fit is refused as
    /// [`PlaybackError::Latency`].
    pub(crate) fn solve(&self, sink: NodeId) -> Result<Solution, PlaybackError> {
        let count = self.nodes.len();
        if sink.0 >= count {
            return Err(PlaybackError::BufferTooSmall);
        }
        // Each node's outgoing links, as one array sliced by `starts`.
        let mut starts = vec![0usize; count + 1];
        let mut waiting = vec![0usize; count];
        for link in &self.links {
            let (Some(start), Some(inputs)) =
                (starts.get_mut(link.from + 1), waiting.get_mut(link.to))
            else {
                return Err(PlaybackError::BufferTooSmall);
            };
            *start += 1;
            *inputs += 1;
        }
        let mut total = 0usize;
        for start in &mut starts {
            total += *start;
            *start = total;
        }
        let mut cursor = starts.clone();
        let mut outgoing = vec![0usize; self.links.len()];
        for (index, link) in self.links.iter().enumerate() {
            let place = cursor
                .get_mut(link.from)
                .ok_or(PlaybackError::BufferTooSmall)?;
            if let Some(slot) = outgoing.get_mut(*place) {
                *slot = index;
            }
            *place += 1;
        }

        let mut lags = vec![0u64; count];
        let mut tails = vec![0u32; count];
        // Nodes whose inputs are all known. Every node enters once.
        let mut ready: Vec<usize> = (0..count)
            .filter(|node| waiting.get(*node) == Some(&0))
            .collect();
        let mut solved = 0usize;
        while let Some(current) = ready.pop() {
            solved += 1;
            let (Some(node), Some(lag), Some(tail)) = (
                self.nodes.get(current),
                lags.get(current).copied(),
                tails.get(current).copied(),
            ) else {
                return Err(PlaybackError::BufferTooSmall);
            };
            let arrival = lag
                .checked_add(node.latency)
                .ok_or(PlaybackError::Latency(u64::MAX))?;
            let tail = tail.saturating_add(node.tail);
            if let Some(slot) = tails.get_mut(current) {
                *slot = tail;
            }
            let (Some(&from), Some(&to)) = (starts.get(current), starts.get(current + 1)) else {
                return Err(PlaybackError::BufferTooSmall);
            };
            for &index in outgoing.get(from..to).unwrap_or_default() {
                let Some(link) = self.links.get(index) else {
                    return Err(PlaybackError::BufferTooSmall);
                };
                let (Some(lag), Some(longest), Some(inputs)) = (
                    lags.get_mut(link.to),
                    tails.get_mut(link.to),
                    waiting.get_mut(link.to),
                ) else {
                    return Err(PlaybackError::BufferTooSmall);
                };
                *lag = (*lag).max(arrival);
                *longest = (*longest).max(tail);
                *inputs -= 1;
                if *inputs == 0 {
                    ready.push(link.to);
                }
            }
        }
        if solved != count {
            return Err(PlaybackError::Feedback);
        }

        let arrival = |node: usize| {
            let lag = lags.get(node)?;
            lag.checked_add(self.nodes.get(node)?.latency)
        };
        let mut delays = Vec::with_capacity(self.links.len());
        for link in &self.links {
            let delay = lags
                .get(link.to)
                .zip(arrival(link.from))
                .and_then(|(lag, arrival)| lag.checked_sub(arrival))
                .ok_or(PlaybackError::Latency(u64::MAX))?;
            delays.push(delay);
        }
        let latency = arrival(sink.0).ok_or(PlaybackError::Latency(u64::MAX))?;
        let tail = tails.get(sink.0).copied().unwrap_or(0);
        Ok(Solution {
            delays,
            lags,
            latency,
            tail,
        })
    }
}

impl Solution {
    /// Frames to hold `link`'s audio so it meets the other inputs of its target.
    pub(crate) fn delay(&self, link: LinkId) -> Result<u64, PlaybackError> {
        self.delays
            .get(link.0)
            .copied()
            .ok_or(PlaybackError::BufferTooSmall)
    }

    /// How far the sink's output is behind the timeline.
    pub(crate) fn latency(&self) -> u64 {
        self.latency
    }

    /// The longest declared tail along any path into the sink.
    pub(crate) fn tail(&self) -> u32 {
        self.tail
    }

    /// Whether these delays align `graph`, by rules that hold link by link
    /// and node by node whatever order they are checked in:
    ///
    /// - every link's source, after its latency and the link's delay, arrives
    ///   exactly at its target's lag, so all inputs of a node meet;
    /// - a node without inputs reads the timeline, and a node with inputs
    ///   waits for none of them longer than its latest one needs;
    /// - the reported latency is the sink's lag after its own latency.
    ///
    /// Together they leave one solution. Control checks every plan before it
    /// is published, so a wrong delay is a refused plan rather than a
    /// misaligned mix.
    pub(crate) fn verify(&self, graph: &Graph, sink: NodeId) -> Result<(), PlaybackError> {
        let wrong = PlaybackError::Misaligned;
        if self.delays.len() != graph.links.len() || self.lags.len() != graph.nodes.len() {
            return Err(wrong);
        }
        let arrival = |node: usize| {
            let lag = self.lags.get(node)?;
            lag.checked_add(graph.nodes.get(node)?.latency)
        };
        // The shortest wait among each node's inputs; `None` without inputs.
        let mut shortest: Vec<Option<u64>> = vec![None; graph.nodes.len()];
        for (link, delay) in graph.links.iter().zip(&self.delays) {
            let arrives = arrival(link.from).and_then(|arrival| arrival.checked_add(*delay));
            if arrives.is_none() || arrives != self.lags.get(link.to).copied() {
                return Err(wrong);
            }
            if let Some(slot) = shortest.get_mut(link.to) {
                *slot = Some(slot.map_or(*delay, |known| known.min(*delay)));
            }
        }
        for (lag, shortest) in self.lags.iter().zip(&shortest) {
            let aligned = match shortest {
                None => *lag == 0,
                Some(wait) => *wait == 0,
            };
            if !aligned {
                return Err(wrong);
            }
        }
        if arrival(sink.0) != Some(self.latency) {
            return Err(wrong);
        }
        Ok(())
    }
}

/// One hosted device as the compensation sees it, read once per plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeviceLatency {
    pub(crate) device: u128,
    /// Reported latency, in device frames.
    pub(crate) latency: u32,
    /// Declared tail. `u32::MAX` is infinite.
    pub(crate) tail: u32,
    pub(crate) instrument: bool,
    pub(crate) bypassed: bool,
}

/// What one track adds to the routing.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TrackFlow<'a> {
    /// Position of the parent whose input this track's output joins. Master
    /// otherwise. Every parent precedes its children.
    pub(crate) parent: Option<usize>,
    /// The track plays placed audio of its own.
    pub(crate) clips: bool,
    /// Its hosted devices in chain order.
    pub(crate) devices: &'a [DeviceLatency],
}

/// Delays for one track's lines, in device frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TrackDelays {
    /// The track's own placed audio, to meet its instrument and children.
    /// Zero without placed audio.
    pub(crate) source: u64,
    /// The first instrument's output, to meet the placed audio and children.
    /// Zero when the chain does not start with an instrument.
    pub(crate) instrument: u64,
    /// The track's output before its fader, to meet its siblings.
    pub(crate) output: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Compensation {
    /// In the order of the tracks given.
    pub(crate) tracks: Vec<TrackDelays>,
    /// The metronome's clicks, to meet the music at master.
    pub(crate) clicks: u64,
    /// How far master lags the timeline.
    pub(crate) latency: u64,
    /// The longest declared tail along any path to master.
    pub(crate) tail: u32,
}

/// Describes the tracks as a [`Graph`], solves it and checks the result.
///
/// A track's placed audio, its first instrument and its children's outputs
/// meet at its input. Its later devices follow in order, and its output
/// joins its parent's input, or master with the metronome.
///
/// A bypassed device passes its audio at once but keeps its place in time:
/// the frames it would take are spent at the end of the track's chain, in the
/// same line as the output's delay. Bypassing a device therefore moves no
/// other track, and the devices after it hear the dry audio that much early.
pub(crate) fn compensate(tracks: &[TrackFlow<'_>]) -> Result<Compensation, PlaybackError> {
    struct Lines {
        source: Option<LinkId>,
        instrument: Option<LinkId>,
        output: LinkId,
        makeup: u64,
    }
    let mut graph = Graph::default();
    let master = graph.node(0, 0);
    let metronome = graph.node(0, 0);
    let clicks = graph.link(metronome, master);
    let mut inputs = Vec::with_capacity(tracks.len());
    let mut lines = Vec::with_capacity(tracks.len());
    for (position, track) in tracks.iter().enumerate() {
        let input = graph.node(0, 0);
        inputs.push(input);
        let source = track.clips.then(|| {
            let clips = graph.node(0, 0);
            graph.link(clips, input)
        });
        let mut devices = track.devices;
        let mut instrument = None;
        if let Some((first, later)) = devices.split_first()
            && first.instrument
        {
            // It hears the notes on the timeline, not the track's input. A
            // bypassed one is silent and keeps its place.
            let tail = if first.bypassed { 0 } else { first.tail };
            let node = graph.node(u64::from(first.latency), tail);
            instrument = Some(graph.link(node, input));
            devices = later;
        }
        let mut output = input;
        let mut makeup = 0u64;
        for device in devices {
            let frames = u64::from(device.latency);
            if device.bypassed {
                makeup = makeup
                    .checked_add(frames)
                    .ok_or(PlaybackError::Latency(u64::MAX))?;
                continue;
            }
            let node = graph.node(frames, device.tail);
            graph.link(output, node);
            output = node;
        }
        if makeup != 0 {
            let node = graph.node(makeup, 0);
            graph.link(output, node);
            output = node;
        }
        let target = match track.parent {
            None => master,
            Some(parent) if parent < position => {
                *inputs.get(parent).ok_or(PlaybackError::BufferTooSmall)?
            }
            Some(_) => return Err(PlaybackError::BufferTooSmall),
        };
        lines.push(Lines {
            source,
            instrument,
            output: graph.link(output, target),
            makeup,
        });
    }
    let solution = graph.solve(master)?;
    solution.verify(&graph, master)?;
    let delay = |link: Option<LinkId>| link.map_or(Ok(0), |link| solution.delay(link));
    let tracks = lines
        .iter()
        .map(|lines| {
            Ok(TrackDelays {
                source: delay(lines.source)?,
                instrument: delay(lines.instrument)?,
                output: solution
                    .delay(lines.output)?
                    .checked_add(lines.makeup)
                    .ok_or(PlaybackError::Latency(u64::MAX))?,
            })
        })
        .collect::<Result<Vec<_>, PlaybackError>>()?;
    Ok(Compensation {
        tracks,
        clicks: solution.delay(clicks)?,
        latency: solution.latency(),
        tail: solution.tail(),
    })
}

#[cfg(test)]
#[path = "compensation_tests.rs"]
mod tests;
