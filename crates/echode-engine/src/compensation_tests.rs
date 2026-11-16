use std::collections::{HashMap, HashSet};

use super::*;
use crate::delay::Delay;

/// A small deterministic generator, so a failure names its seed.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }

    /// Mostly zero, as most devices are, otherwise up to a few blocks.
    fn latency(&mut self) -> u64 {
        match self.below(3) {
            0 => 0,
            1 => self.below(8),
            _ => self.below(700),
        }
    }
}

/// A random graph without loops. `order` lists the nodes sources first; the
/// node numbers themselves are shuffled, so nothing may rely on them.
struct Fixture {
    graph: Graph,
    order: Vec<usize>,
    sink: NodeId,
}

fn fixture(random: &mut Random, nodes: usize) -> Fixture {
    let mut order: Vec<usize> = (0..nodes).collect();
    for index in (1..nodes).rev() {
        order.swap(index, random.below(index as u64 + 1) as usize);
    }
    let mut latencies = vec![(0u64, 0u32); nodes];
    for slot in &mut latencies {
        *slot = (random.latency(), random.below(50) as u32);
    }
    let mut graph = Graph::default();
    for (latency, tail) in &latencies {
        graph.node(*latency, *tail);
    }
    for rank in 1..nodes {
        // Up to three inputs, twice from the same node now and then.
        for _ in 0..random.below(4) {
            let from = order[random.below(rank as u64) as usize];
            graph.link(NodeId(from), NodeId(order[rank]));
        }
    }
    // Everything without a listener is heard at the sink, like master.
    let sink = graph.node(random.latency(), 0);
    let mut heard = vec![false; nodes];
    for link in &graph.links {
        heard[link.from] = true;
    }
    for (node, heard) in heard.iter().enumerate() {
        if !heard {
            graph.link(NodeId(node), sink);
        }
    }
    order.push(sink.0);
    Fixture { graph, order, sink }
}

/// Every path from a timeline reader to `sink`, as `(frames, tail)`: the
/// latencies and the solved delays along it, and its declared tails.
fn paths(graph: &Graph, solution: &Solution, sink: NodeId) -> Vec<(u64, u32)> {
    let mut found = Vec::new();
    let sources =
        (0..graph.nodes.len()).filter(|node| graph.links.iter().all(|link| link.to != *node));
    let mut stack: Vec<(usize, u64, u32)> = sources.map(|node| (node, 0, 0)).collect();
    while let Some((node, frames, tail)) = stack.pop() {
        let frames = frames + graph.nodes[node].latency;
        let tail = tail.saturating_add(graph.nodes[node].tail);
        if node == sink.0 {
            found.push((frames, tail));
        }
        for (index, link) in graph.links.iter().enumerate() {
            if link.from == node {
                stack.push((link.to, frames + solution.delays[index], tail));
            }
        }
    }
    found
}

/// Plays one impulse from every timeline reader through real delay lines, in
/// blocks of `block`, and returns what the sink puts out.
fn play(fixture: &Fixture, solution: &Solution, block: usize, frames: usize) -> Vec<f32> {
    let graph = &fixture.graph;
    let mut lines: Vec<Delay> = solution
        .delays
        .iter()
        .map(|delay| Delay::new(*delay, block).unwrap())
        .collect();
    let mut processing: Vec<Delay> = graph
        .nodes
        .iter()
        .map(|node| Delay::new(node.latency, block).unwrap())
        .collect();
    let mut outputs = vec![vec![0.0f32; block]; graph.nodes.len()];
    let mut heard = Vec::new();
    let mut scratch = vec![0.0f32; block];
    while heard.len() < frames {
        for &node in &fixture.order {
            let mut input = vec![0.0f32; block];
            let mut reads_timeline = true;
            for (index, link) in graph.links.iter().enumerate() {
                if link.to != node {
                    continue;
                }
                reads_timeline = false;
                scratch.copy_from_slice(&outputs[link.from]);
                lines[index].process(&mut scratch, None);
                for (sum, sample) in input.iter_mut().zip(&scratch) {
                    *sum += *sample;
                }
            }
            if reads_timeline && heard.is_empty() {
                input[0] = 1.0;
            }
            processing[node].process(&mut input, None);
            outputs[node] = input;
        }
        heard.extend_from_slice(&outputs[fixture.sink.0]);
    }
    heard.truncate(frames);
    heard
}

#[test]
fn inputs_meet_at_the_latest_one() {
    let mut graph = Graph::default();
    let (fast, slow, sink) = (graph.node(10, 0), graph.node(70, 0), graph.node(5, 0));
    let (early, late) = (graph.link(fast, sink), graph.link(slow, sink));
    let solution = graph.solve(sink).unwrap();
    assert_eq!(solution.delay(early).unwrap(), 60);
    assert_eq!(solution.delay(late).unwrap(), 0);
    assert_eq!(solution.latency(), 75);
    assert_eq!(solution.lags, [0, 0, 70]);
    solution.verify(&graph, sink).unwrap();
}

#[test]
fn one_output_heard_twice_is_aligned_at_both_places() {
    // A send: `dry` goes to master and through `reverb`, which is later.
    let mut graph = Graph::default();
    let (dry, reverb, master) = (graph.node(0, 0), graph.node(256, 9), graph.node(0, 0));
    let direct = graph.link(dry, master);
    let send = graph.link(dry, reverb);
    let back = graph.link(reverb, master);
    let solution = graph.solve(master).unwrap();
    assert_eq!(
        [direct, send, back].map(|link| solution.delay(link).unwrap()),
        [256, 0, 0]
    );
    assert_eq!((solution.latency(), solution.tail()), (256, 9));
}

#[test]
fn a_loop_is_refused() {
    let mut graph = Graph::default();
    let (first, second, sink) = (graph.node(1, 0), graph.node(1, 0), graph.node(0, 0));
    graph.link(first, second);
    graph.link(second, sink);
    assert!(graph.solve(sink).is_ok());
    graph.link(second, first);
    assert!(matches!(graph.solve(sink), Err(PlaybackError::Feedback)));

    let mut alone = Graph::default();
    let node = alone.node(0, 0);
    alone.link(node, node);
    assert!(matches!(alone.solve(node), Err(PlaybackError::Feedback)));
}

#[test]
fn a_sum_that_does_not_fit_or_a_missing_node_is_refused() {
    let mut graph = Graph::default();
    let (huge, more) = (graph.node(u64::MAX, 0), graph.node(1, 0));
    graph.link(huge, more);
    assert!(matches!(
        graph.solve(more),
        Err(PlaybackError::Latency(u64::MAX))
    ));
    assert!(matches!(
        graph.solve(NodeId(2)),
        Err(PlaybackError::BufferTooSmall)
    ));

    let mut dangling = Graph::default();
    let node = dangling.node(0, 0);
    dangling.link(node, NodeId(7));
    assert!(matches!(
        dangling.solve(node),
        Err(PlaybackError::BufferTooSmall)
    ));
}

#[test]
fn deep_and_wide_routings_are_solved_without_recursion() {
    // These sizes only bound this fixture; the routing has no limit.
    let count: usize = 100_000;
    let mut deep = Graph::default();
    let mut last = deep.node(1, 1);
    for _ in 1..count {
        let next = deep.node(1, 1);
        deep.link(last, next);
        last = next;
    }
    let solution = deep.solve(last).unwrap();
    assert_eq!(solution.latency(), count as u64);
    assert_eq!(solution.tail(), count as u32);
    assert!(solution.delays.iter().all(|delay| *delay == 0));
    solution.verify(&deep, last).unwrap();

    let mut wide = Graph::default();
    let sink = wide.node(0, 0);
    let links: Vec<_> = (0..count)
        .map(|index| {
            let source = wide.node(index as u64, 0);
            wide.link(source, sink)
        })
        .collect();
    let solution = wide.solve(sink).unwrap();
    assert_eq!(solution.latency(), count as u64 - 1);
    assert_eq!(solution.delay(links[0]).unwrap(), count as u64 - 1);
    solution.verify(&wide, sink).unwrap();
}

#[test]
fn the_check_refuses_every_kind_of_wrong_delay() {
    let mut graph = Graph::default();
    let (fast, slow, sink) = (graph.node(10, 0), graph.node(70, 0), graph.node(5, 0));
    graph.link(fast, sink);
    graph.link(slow, sink);
    let solution = graph.solve(sink).unwrap();
    let wrong = |change: &dyn Fn(&mut Solution)| {
        let mut other = Solution {
            delays: solution.delays.clone(),
            lags: solution.lags.clone(),
            latency: solution.latency,
            tail: solution.tail,
        };
        change(&mut other);
        matches!(other.verify(&graph, sink), Err(PlaybackError::Misaligned))
    };
    // One input early; every input late by the same amount; a source that
    // does not read the timeline; another latency; a missing line.
    assert!(wrong(&|other| other.delays[0] -= 1));
    assert!(wrong(&|other| {
        other.delays = vec![61, 1];
        other.lags[2] = 71;
        other.latency = 76;
    }));
    assert!(wrong(&|other| {
        other.lags[0] = 1;
        other.delays[0] = 59;
    }));
    assert!(wrong(&|other| other.latency += 1));
    assert!(wrong(&|other| other.delays.truncate(1)));
    assert!(!wrong(&|_| {}));
}

#[test]
fn every_path_of_a_random_routing_takes_the_same_time() {
    for seed in 1..=400u64 {
        let mut random = Random(seed);
        let nodes = 1 + random.below(9) as usize;
        let fixture = fixture(&mut random, nodes);
        let solution = fixture.graph.solve(fixture.sink).unwrap();
        solution
            .verify(&fixture.graph, fixture.sink)
            .unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        let paths = paths(&fixture.graph, &solution, fixture.sink);
        assert!(!paths.is_empty(), "seed {seed}");
        for (frames, _) in &paths {
            assert_eq!(*frames, solution.latency(), "seed {seed}");
        }
        let longest = paths.iter().map(|path| path.1).max().unwrap();
        assert_eq!(solution.tail(), longest, "seed {seed}");
        // No smaller latency aligns the sink: one path waits nowhere.
        let unsolved = Solution {
            delays: vec![0; solution.delays.len()],
            ..fixture.graph.solve(fixture.sink).unwrap()
        };
        let shortest_wait = self::paths(&fixture.graph, &unsolved, fixture.sink)
            .iter()
            .map(|path| path.0)
            .max();
        assert_eq!(shortest_wait, Some(solution.latency()), "seed {seed}");
    }
}

#[test]
fn impulses_through_real_lines_arrive_together_for_every_block_size() {
    for seed in 1..=60u64 {
        let mut random = Random(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let nodes = 1 + random.below(8) as usize;
        let fixture = fixture(&mut random, nodes);
        let solution = fixture.graph.solve(fixture.sink).unwrap();
        let arrivals = paths(&fixture.graph, &solution, fixture.sink).len() as f32;
        let at = solution.latency() as usize;
        for block in [1usize, 7, 64, 512] {
            if block == 1 && at > 600 {
                // One frame per block over a long routing only costs time.
                continue;
            }
            let heard = play(&fixture, &solution, block, at + 2 * block + 1);
            for (frame, sample) in heard.iter().enumerate() {
                let expected = if frame == at { arrivals } else { 0.0 };
                assert_eq!(
                    *sample, expected,
                    "seed {seed}, block {block}, frame {frame}"
                );
            }
        }
    }
}

fn device(latency: u32) -> DeviceLatency {
    DeviceLatency {
        device: 0,
        latency,
        tail: 0,
        instrument: false,
        bypassed: false,
    }
}

fn instrument(latency: u32) -> DeviceLatency {
    DeviceLatency {
        instrument: true,
        ..device(latency)
    }
}

fn flow<'a>(parent: Option<usize>, clips: bool, devices: &'a [DeviceLatency]) -> TrackFlow<'a> {
    TrackFlow {
        parent,
        clips,
        devices,
        notes_from: None,
    }
}

/// Plans tracks whose only merges are their folders.
fn tracks_only(flows: &[TrackFlow<'_>]) -> Result<Compensation, PlaybackError> {
    compensate(Routes {
        tracks: flows,
        ..Routes::default()
    })
}

fn delays(source: u64, instrument: u64, output: u64) -> TrackDelays {
    TrackDelays {
        source,
        instrument,
        output,
    }
}

#[test]
fn device_input_times_come_from_the_audio_graph_including_bypass_and_folders() {
    let identified = |id, mut device: DeviceLatency| {
        device.device = id;
        device
    };
    let mut bypass = identified(4, device(11));
    bypass.bypassed = true;
    let parent = [identified(1, instrument(7)), identified(2, device(5))];
    let child = [identified(3, device(37)), bypass, identified(5, device(13))];
    let plan = tracks_only(&[flow(None, true, &parent), flow(Some(0), false, &child)]).unwrap();
    // The child takes 61 including bypass makeup, so the parent's instrument
    // output waits until 61. The bypass does not delay the next active FX input.
    assert_eq!(plan.stage_inputs, vec![(1, 0), (2, 61), (3, 0), (5, 37)]);
    assert_eq!(plan.latency, 66);
}

#[test]
fn top_level_tracks_wait_for_the_latest_one_at_master() {
    let chains = [[device(100)], [device(0)], [device(30)]];
    let flows: Vec<_> = chains.iter().map(|chain| flow(None, true, chain)).collect();
    let plan = tracks_only(&flows).unwrap();
    assert_eq!(plan.latency, 100);
    assert_eq!(plan.clicks, 100);
    assert_eq!(
        plan.tracks,
        [delays(0, 0, 0), delays(0, 0, 100), delays(0, 0, 70)]
    );
}

#[test]
fn children_meet_at_their_parent_before_its_devices() {
    // root{fast, slow} with its own device, then a track outside the folder.
    let chains = [[device(20)], [device(10)], [device(50)], [device(0)]];
    let flows = [
        flow(None, true, &chains[0]),
        flow(Some(0), true, &chains[1]),
        flow(Some(0), true, &chains[2]),
        flow(None, true, &chains[3]),
    ];
    let plan = tracks_only(&flows).unwrap();
    // The root's clips wait for its slowest child; the root arrives 50 + 20.
    assert_eq!(plan.latency, 70);
    assert_eq!(
        plan.tracks,
        [
            delays(50, 0, 0),
            delays(0, 0, 40),
            delays(0, 0, 0),
            delays(0, 0, 70)
        ]
    );
}

#[test]
fn a_folder_instrument_waits_for_slower_children_and_they_wait_for_it() {
    // The instrument is later than the child: the child and the clips wait.
    let slow = [instrument(64), device(8)];
    let child = [device(16)];
    let plan = tracks_only(&[flow(None, true, &slow), flow(Some(0), false, &child)]).unwrap();
    assert_eq!(plan.tracks, [delays(64, 0, 0), delays(0, 0, 48)]);
    assert_eq!(plan.latency, 72);
    // The child is later than the instrument: the instrument waits.
    let fast = [instrument(4), device(8)];
    let plan = tracks_only(&[flow(None, true, &fast), flow(Some(0), false, &child)]).unwrap();
    assert_eq!(plan.tracks, [delays(16, 12, 0), delays(0, 0, 0)]);
    assert_eq!(plan.latency, 24);
}

#[test]
fn a_line_exists_only_where_audio_flows() {
    // No placed audio and no instrument: nothing to delay at the input.
    let child = [device(16)];
    let plan = tracks_only(&[flow(None, false, &[]), flow(Some(0), false, &child)]).unwrap();
    assert_eq!(plan.tracks, [delays(0, 0, 0), delays(0, 0, 0)]);
    // A track that plays nothing still joins master in time.
    let plan = tracks_only(&[flow(None, false, &[]), flow(None, false, &child)]).unwrap();
    assert_eq!(plan.tracks, [delays(0, 0, 16), delays(0, 0, 0)]);
}

#[test]
fn a_bypassed_device_keeps_its_place_and_delays_its_own_output() {
    let bypassed = [DeviceLatency {
        bypassed: true,
        ..device(100)
    }];
    let plan = tracks_only(&[flow(None, true, &bypassed), flow(None, true, &[])]).unwrap();
    assert_eq!(plan.latency, 100);
    // Its dry audio is early by the device's latency; the other track still
    // waits as if the device ran.
    assert_eq!(plan.tracks, [delays(0, 0, 100), delays(0, 0, 100)]);
    // A bypassed first instrument is silent and keeps its place too.
    let silent = [DeviceLatency {
        bypassed: true,
        tail: 500,
        ..instrument(40)
    }];
    let plan = tracks_only(&[flow(None, true, &silent)]).unwrap();
    assert_eq!(plan.tracks, [delays(40, 0, 0)]);
    assert_eq!((plan.latency, plan.tail), (40, 0));
}

#[test]
fn tails_add_along_each_path_to_master() {
    let tailed = |tail: u32| [DeviceLatency { tail, ..device(0) }];
    let chains = [tailed(100), tailed(20), tailed(3), tailed(110)];
    let parents = [None, Some(0), Some(1), None];
    let flows: Vec<_> = chains
        .iter()
        .zip(parents)
        .map(|(chain, parent)| flow(parent, true, chain))
        .collect();
    assert_eq!(tracks_only(&flows).unwrap().tail, 123);
    let chains = [tailed(u32::MAX), tailed(1), tailed(1), tailed(0)];
    let flows: Vec<_> = chains
        .iter()
        .zip(parents)
        .map(|(chain, parent)| flow(parent, true, chain))
        .collect();
    assert_eq!(tracks_only(&flows).unwrap().tail, u32::MAX);
    // A child's audio does not pass its parent's instrument.
    let parent = [
        DeviceLatency {
            tail: 1_000,
            ..instrument(0)
        },
        tailed(5)[0],
    ];
    let child = tailed(40);
    let plan = tracks_only(&[flow(None, false, &parent), flow(Some(0), true, &child)]).unwrap();
    assert_eq!(plan.tail, 1_005);
}

#[test]
fn a_deep_folder_chain_is_planned_without_recursion() {
    // The depth only sizes this fixture; folders have no depth limit.
    let depth: usize = 10_000;
    let chain = [device(1)];
    let flows: Vec<_> = (0..depth)
        .map(|position| flow(position.checked_sub(1), true, &chain))
        .collect();
    let plan = tracks_only(&flows).unwrap();
    assert_eq!(plan.latency, depth as u64);
    assert!(plan.tracks.iter().all(|track| track.output == 0));
    assert_eq!(plan.tracks[0].source, depth as u64 - 1);
}

#[test]
fn latencies_past_32_bits_add_up_and_folders_come_in_any_order() {
    // Each device reports 32 bits; two in a row already pass that.
    let huge = vec![
        DeviceLatency {
            bypassed: true,
            ..device(u32::MAX)
        };
        2
    ];
    let plan = tracks_only(&[flow(None, true, &huge)]).unwrap();
    assert_eq!(plan.latency, 2 * u64::from(u32::MAX));
    // Routes may point at later tracks, so a parent after its child is a
    // folder like any other; a track in its own folder is a loop.
    let slow = [device(9)];
    let plan = tracks_only(&[flow(Some(1), true, &slow), flow(None, true, &[])]).unwrap();
    assert_eq!(plan.tracks, [delays(0, 0, 0), delays(9, 0, 0)]);
    assert!(matches!(
        tracks_only(&[flow(Some(0), true, &[])]),
        Err(PlaybackError::Feedback)
    ));
    assert!(matches!(
        tracks_only(&[flow(Some(1), true, &[]), flow(Some(0), true, &[])]),
        Err(PlaybackError::Feedback)
    ));
    assert!(matches!(
        tracks_only(&[flow(Some(1), true, &[])]),
        Err(PlaybackError::BufferTooSmall)
    ));
}

fn with_id(id: u128, device: DeviceLatency) -> DeviceLatency {
    DeviceLatency {
        device: id,
        ..device
    }
}

fn send(from: usize, to: usize) -> SendFlow {
    SendFlow { from, to }
}

fn sidechain(from: usize, track: usize, device: usize) -> SidechainFlow {
    SidechainFlow {
        from,
        track,
        device,
    }
}

fn routed(
    tracks: &[TrackFlow<'_>],
    sends: &[SendFlow],
    sidechains: &[SidechainFlow],
) -> Result<Compensation, PlaybackError> {
    compensate(Routes {
        tracks,
        sends,
        sidechains,
    })
}

#[test]
fn a_send_meets_its_return_and_the_dry_tracks_wait_for_the_return() {
    // Two dry tracks send to a return whose reverb takes 256 frames.
    let (dry, reverb, other) = ([device(100)], [device(256)], [device(0)]);
    let flows = [
        flow(None, true, &dry),
        flow(None, false, &reverb),
        flow(None, true, &other),
    ];
    let plan = routed(&flows, &[send(0, 1), send(2, 1)], &[]).unwrap();
    // The return hears both sends at 100 and reaches master at 356.
    assert_eq!(plan.sends, [0, 100]);
    assert_eq!(
        plan.tracks,
        [delays(0, 0, 256), delays(0, 0, 0), delays(0, 0, 356)]
    );
    assert_eq!(plan.latency, 356);
}

#[test]
fn a_send_from_a_bypassed_chain_holds_the_bypassed_frames() {
    let bypassed = [DeviceLatency {
        bypassed: true,
        ..device(50)
    }];
    let flows = [flow(None, true, &bypassed), flow(None, false, &[])];
    let plan = routed(&flows, &[send(0, 1)], &[]).unwrap();
    // The dry audio leaves 50 early; the send holds that as the output does.
    assert_eq!(plan.sends, [50]);
    assert_eq!(plan.tracks, [delays(0, 0, 50), delays(0, 0, 0)]);
    assert_eq!(plan.latency, 50);
}

#[test]
fn a_sidechain_holds_the_chain_where_it_joins() {
    // A kick with a 300-frame device keys the bass's compressor after its EQ.
    let kick = [device(300)];
    let bass = [device(20), with_id(7, device(10))];
    let flows = [flow(None, true, &kick), flow(None, true, &bass)];
    let plan = routed(&flows, &[], &[sidechain(0, 1, 1)]).unwrap();
    assert_eq!(
        plan.stage_lines,
        [StageLine {
            track: 1,
            device: 1,
            main: 280
        }]
    );
    assert_eq!(plan.sidechains, [0]);
    assert!(plan.stage_inputs.contains(&(7, 300)));
    assert_eq!(plan.tracks, [delays(0, 0, 10), delays(0, 0, 0)]);
    assert_eq!(plan.latency, 310);
}

#[test]
fn bypassing_a_device_before_or_at_a_sidechain_moves_nothing_else() {
    let key = [device(50)];
    let chain = |first: bool, second: bool| {
        [
            DeviceLatency {
                bypassed: first,
                ..device(100)
            },
            DeviceLatency {
                bypassed: second,
                ..device(30)
            },
        ]
    };
    let plan_for = |chain: &[DeviceLatency]| {
        routed(
            &[flow(None, true, &key), flow(None, true, chain)],
            &[],
            &[sidechain(0, 1, 1)],
        )
        .unwrap()
    };
    let active = plan_for(&chain(false, false));
    // The key arrives at 50 and the chain at 100, so the key waits 50.
    assert_eq!(active.latency, 130);
    assert_eq!(active.sidechains, [50]);
    assert_eq!(active.stage_lines[0].main, 0);
    for (first, second) in [(true, false), (false, true), (true, true)] {
        let plan = plan_for(&chain(first, second));
        assert_eq!(plan.latency, 130);
        assert_eq!(plan.sidechains, [50]);
        assert_eq!(plan.tracks[0], active.tracks[0]);
        // The dry audio is early by what it skipped; the next line holds it.
        assert_eq!(plan.stage_lines[0].main, if first { 100 } else { 0 });
        assert_eq!(plan.tracks[1].output, if second { 30 } else { 0 });
    }
}

#[test]
fn a_drum_rack_hands_its_notes_to_the_pads_without_a_false_loop() {
    // The folder holds the notes; each pad's instrument plays them, and the
    // pads sum back into the folder.
    let kick = [with_id(1, instrument(30))];
    let snare = [with_id(2, instrument(5))];
    let bus = [with_id(3, device(8))];
    let pad = |devices| TrackFlow {
        notes_from: Some(0),
        ..flow(Some(0), false, devices)
    };
    let flows = [flow(None, false, &bus), pad(&kick), pad(&snare)];
    let plan = tracks_only(&flows).unwrap();
    assert_eq!(
        plan.tracks,
        [delays(0, 0, 0), delays(0, 0, 0), delays(0, 0, 25)]
    );
    assert_eq!(plan.stage_inputs, [(3, 30), (1, 0), (2, 0)]);
    assert_eq!(plan.latency, 38);
    // A pad keyed by a later track hears the folder's notes at that time.
    let room = [with_id(4, device(40))];
    let flows = [
        flow(None, false, &bus),
        pad(&kick),
        pad(&snare),
        flow(None, true, &room),
    ];
    let plan = routed(&flows, &[], &[sidechain(3, 1, 0)]).unwrap();
    assert_eq!(plan.stage_inputs, [(3, 70), (1, 40), (2, 0), (4, 0)]);
    assert!(plan.stage_lines.is_empty());
    assert_eq!(plan.tracks[2].output, 65);
    assert_eq!(plan.latency, 78);
}

#[test]
fn routes_that_feed_back_are_refused_and_missing_ends_are_errors() {
    let chain = [device(1)];
    let two = [flow(None, true, &chain), flow(None, true, &chain)];
    let folder = [flow(None, true, &chain), flow(Some(0), true, &chain)];
    // Tracks sending to each other, a send to itself, a folder sending to its
    // child, a sidechain keyed by its own track or by its own send.
    for result in [
        routed(&two, &[send(0, 1), send(1, 0)], &[]),
        routed(&two, &[send(1, 1)], &[]),
        routed(&folder, &[send(0, 1)], &[]),
        routed(&two, &[], &[sidechain(1, 1, 0)]),
        routed(&two, &[send(0, 1)], &[sidechain(1, 0, 0)]),
    ] {
        assert!(matches!(result, Err(PlaybackError::Feedback)), "{result:?}");
    }
    // A child sending to its folder is two paths, not a loop.
    assert!(routed(&folder, &[send(1, 0)], &[]).is_ok());
    let player = [TrackFlow {
        notes_from: Some(2),
        ..flow(None, false, &chain)
    }];
    for result in [
        routed(&two, &[send(0, 2)], &[]),
        routed(&two, &[send(2, 0)], &[]),
        routed(&two, &[], &[sidechain(2, 0, 0)]),
        routed(&two, &[], &[sidechain(0, 2, 0)]),
        routed(&two, &[], &[sidechain(0, 1, 1)]),
        routed(&player, &[], &[]),
    ] {
        assert!(
            matches!(result, Err(PlaybackError::BufferTooSmall)),
            "{result:?}"
        );
    }
}

#[test]
fn the_memory_budget_sees_every_line_of_a_plan() {
    let (kick, bass, reverb) = ([device(300)], [device(20), device(10)], [device(9)]);
    let flows = [
        flow(None, true, &kick),
        flow(None, true, &bass),
        flow(None, false, &reverb),
    ];
    let plan = routed(&flows, &[send(1, 2)], &[sidechain(0, 1, 1)]).unwrap();
    let mut lines: Vec<u64> = plan.lines().collect();
    let mut expected: Vec<u64> = plan
        .tracks
        .iter()
        .flat_map(|delays| [delays.source, delays.instrument, delays.output])
        .collect();
    expected.extend([plan.sends[0], plan.sidechains[0], plan.stage_lines[0].main]);
    expected.push(plan.clicks);
    lines.sort_unstable();
    expected.sort_unstable();
    assert_eq!(lines, expected);
    assert!(plan.stage_lines[0].main > 0);
}

/// What the callback does with the lines, as plain delays, for routes in any
/// order. `order` lists each track after every track it hears.
///
/// A track's input hears its placed audio through `source`, its first
/// instrument through its latency and `instrument`, each child's chain output
/// through that child's `output`, and each send through its line. Its later
/// devices follow in order; one keyed by a sidechain hears the chain through
/// its stage line and the key through the sidechain's line. A bypassed device
/// passes the chain's audio at once and drops its key. Notes reach an
/// instrument at its stage's lag, as the note cursor delivers them.
///
/// Panics where the inputs of one merge do not meet, or where a device hears
/// audio at another time than the lag its notes and transport use. Returns
/// when each impulse from the timeline reaches master.
fn arrivals(routes: Routes<'_>, plan: &Compensation, order: &[usize]) -> Vec<u64> {
    let tracks = routes.tracks;
    let lags: HashMap<u128, u64> = plan.stage_inputs.iter().copied().collect();
    let meet = |at: &[u64], lag: Option<u64>, what: &str| {
        let Some(first) = at.first() else {
            return;
        };
        assert!(at.iter().all(|time| time == first), "{what}: {at:?}");
        if let Some(lag) = lag {
            assert_eq!(*first, lag, "{what}: heard at another time than its lag");
        }
    };
    let heard = |taps: &[Option<Vec<u64>>], from: usize, line: u64| -> Vec<u64> {
        taps[from]
            .as_ref()
            .expect("a track plays after what it hears")
            .iter()
            .map(|time| time + line)
            .collect()
    };
    let mut taps: Vec<Option<Vec<u64>>> = vec![None; tracks.len()];
    let mut master = vec![plan.clicks];
    for &position in order {
        let track = &tracks[position];
        let delays = plan.tracks[position];
        let keys = |taps: &[Option<Vec<u64>>], index: usize| -> Vec<u64> {
            routes
                .sidechains
                .iter()
                .zip(&plan.sidechains)
                .filter(|(key, _)| key.track == position && key.device == index)
                .flat_map(|(key, line)| heard(taps, key.from, *line))
                .collect()
        };
        let mut at_input = Vec::new();
        if track.clips {
            at_input.push(delays.source);
        }
        for (child, flow) in tracks.iter().enumerate() {
            if flow.parent == Some(position) {
                at_input.extend(heard(&taps, child, plan.tracks[child].output));
            }
        }
        for (send, line) in routes.sends.iter().zip(&plan.sends) {
            if send.to == position {
                at_input.extend(heard(&taps, send.from, *line));
            }
        }
        let mut later = track.devices.iter().enumerate().peekable();
        if let Some((_, first)) = later.next_if(|(_, device)| device.instrument)
            && !first.bypassed
        {
            let lag = lags[&first.device];
            meet(&keys(&taps, 0), Some(lag), "keyed instrument");
            at_input.push(lag + u64::from(first.latency) + delays.instrument);
        }
        meet(&at_input, None, "track input");
        let mut chain = at_input;
        for (index, device) in later {
            let keyed = routes
                .sidechains
                .iter()
                .any(|key| key.track == position && key.device == index);
            let lag = (!device.bypassed).then(|| lags[&device.device]);
            let mut key = Vec::new();
            if keyed {
                let main = plan
                    .stage_lines
                    .iter()
                    .find(|line| line.track == position && line.device == index)
                    .expect("a keyed device has a main line")
                    .main;
                for time in &mut chain {
                    *time += main;
                }
                key = keys(&taps, index);
                let mut all = chain.clone();
                all.extend(&key);
                meet(&all, lag, "keyed device");
            } else {
                meet(&chain, lag, "device");
            }
            if device.bypassed {
                continue;
            }
            chain.extend(key);
            for time in &mut chain {
                *time += u64::from(device.latency);
            }
        }
        if track.parent.is_none() {
            master.extend(chain.iter().map(|time| time + delays.output));
        }
        taps[position] = Some(chain);
    }
    master
}

/// Tracks with folders, sends, sidechains and handed-out notes, at shuffled
/// positions. Routes only run from a lower rank to a higher one, so there is
/// no loop; notes come from any track, as they never make one.
struct RandomRouting {
    chains: Vec<Vec<DeviceLatency>>,
    parents: Vec<Option<usize>>,
    clips: Vec<bool>,
    notes: Vec<Option<usize>>,
    sends: Vec<SendFlow>,
    sidechains: Vec<SidechainFlow>,
    /// Every track after every track it hears.
    order: Vec<usize>,
}

impl RandomRouting {
    fn new(random: &mut Random) -> Self {
        let count = 1 + random.below(12) as usize;
        let chains: Vec<Vec<DeviceLatency>> = (0..count)
            .map(|position| {
                (0..random.below(4))
                    .map(|index| DeviceLatency {
                        // Unique, so a device's lag can be looked up.
                        device: (position as u128) << 32 | index as u128,
                        latency: random.latency() as u32,
                        tail: 0,
                        instrument: index == 0 && random.below(2) == 0,
                        bypassed: random.below(4) == 0,
                    })
                    .collect()
            })
            .collect();
        let parents: Vec<Option<usize>> = (0..count)
            .map(|position| {
                (position != 0 && random.below(3) != 0)
                    .then(|| random.below(position as u64) as usize)
            })
            .collect();
        // A parent hears its children, so it ranks above each of them.
        let mut ranks: Vec<u64> = (0..count).map(|_| random.below(64)).collect();
        for position in (0..count).rev() {
            if let Some(parent) = parents[position] {
                ranks[parent] = ranks[parent].max(ranks[position] + 1);
            }
        }
        let pair = |random: &mut Random| {
            let (first, second) = (
                random.below(count as u64) as usize,
                random.below(count as u64) as usize,
            );
            match ranks[first].cmp(&ranks[second]) {
                std::cmp::Ordering::Less => Some((first, second)),
                std::cmp::Ordering::Greater => Some((second, first)),
                std::cmp::Ordering::Equal => None,
            }
        };
        let sends: Vec<SendFlow> = (0..random.below(4))
            .filter_map(|_| pair(random).map(|(from, to)| send(from, to)))
            .collect();
        let sidechains: Vec<SidechainFlow> = (0..random.below(3))
            .filter_map(|_| {
                let (from, track) = pair(random)?;
                let devices = chains[track].len() as u64;
                (devices != 0).then(|| sidechain(from, track, random.below(devices) as usize))
            })
            .collect();
        let clips = (0..count).map(|_| random.below(3) != 0).collect::<Vec<_>>();
        let notes: Vec<Option<usize>> = (0..count)
            .map(|_| (random.below(3) == 0).then(|| random.below(count as u64) as usize))
            .collect();
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by_key(|position| ranks[*position]);

        // Shuffle the positions, so nothing relies on parents coming first.
        let mut moved: Vec<usize> = (0..count).collect();
        for index in (1..count).rev() {
            moved.swap(index, random.below(index as u64 + 1) as usize);
        }
        let mut shuffled = Self {
            chains: vec![Vec::new(); count],
            parents: vec![None; count],
            clips: vec![false; count],
            notes: vec![None; count],
            sends: sends
                .iter()
                .map(|route| send(moved[route.from], moved[route.to]))
                .collect(),
            sidechains: sidechains
                .iter()
                .map(|route| sidechain(moved[route.from], moved[route.track], route.device))
                .collect(),
            order: order.iter().map(|position| moved[*position]).collect(),
        };
        for (position, chain) in chains.into_iter().enumerate() {
            let to = moved[position];
            shuffled.chains[to] = chain;
            shuffled.parents[to] = parents[position].map(|parent| moved[parent]);
            shuffled.clips[to] = clips[position];
            shuffled.notes[to] = notes[position].map(|from| moved[from]);
        }
        shuffled
    }

    fn flows(&self) -> Vec<TrackFlow<'_>> {
        (0..self.chains.len())
            .map(|position| TrackFlow {
                parent: self.parents[position],
                clips: self.clips[position],
                devices: &self.chains[position],
                notes_from: self.notes[position],
            })
            .collect()
    }

    fn routes<'a>(&'a self, flows: &'a [TrackFlow<'a>]) -> Routes<'a> {
        Routes {
            tracks: flows,
            sends: &self.sends,
            sidechains: &self.sidechains,
        }
    }
}

#[test]
fn random_routings_reach_master_together_through_the_callbacks_lines() {
    for seed in 1..=400u64 {
        let mut random = Random(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93));
        let routing = RandomRouting::new(&mut random);
        let flows = routing.flows();
        let routes = routing.routes(&flows);
        let plan = compensate(routes).unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        // A keyed first instrument hears notes, not the chain: it has no main line.
        let keyed: HashSet<_> = routing
            .sidechains
            .iter()
            .filter(|key| key.device != 0 || !routing.chains[key.track][0].instrument)
            .map(|key| (key.track, key.device))
            .collect();
        assert_eq!(plan.stage_lines.len(), keyed.len(), "seed {seed}");
        for arrival in arrivals(routes, &plan, &routing.order) {
            assert_eq!(arrival, plan.latency, "seed {seed}: {routes:?}");
        }
    }
}

#[test]
fn bypassing_any_device_of_a_random_routing_moves_no_other_track() {
    for seed in 1..=300u64 {
        let mut random = Random(seed.wrapping_mul(0x2545_F491_4F6C_DD1D));
        let mut routing = RandomRouting::new(&mut random);
        let before = compensate(routing.routes(&routing.flows())).unwrap();
        let candidates: Vec<usize> = (0..routing.chains.len())
            .filter(|position| !routing.chains[*position].is_empty())
            .collect();
        let Some(&track) = candidates.get(random.below(candidates.len() as u64) as usize) else {
            continue;
        };
        let index = random.below(routing.chains[track].len() as u64) as usize;
        let toggled = &mut routing.chains[track][index];
        toggled.bypassed = !toggled.bypassed;
        let own: HashSet<u128> = routing.chains[track]
            .iter()
            .map(|device| device.device)
            .collect();
        let flows = routing.flows();
        let routes = routing.routes(&flows);
        let after = compensate(routes).unwrap();
        for arrival in arrivals(routes, &after, &routing.order) {
            assert_eq!(arrival, after.latency, "seed {seed}");
        }
        assert_eq!(
            (after.latency, after.clicks),
            (before.latency, before.clicks),
            "seed {seed}"
        );
        for (position, (old, new)) in before.tracks.iter().zip(&after.tracks).enumerate() {
            if position != track {
                assert_eq!(old, new, "seed {seed}: track {position}");
            }
        }
        let from_others = |plan: &Compensation| -> Vec<u64> {
            let sends = routing.sends.iter().zip(&plan.sends);
            let keys = routing
                .sidechains
                .iter()
                .map(|key| key.from)
                .zip(&plan.sidechains);
            sends
                .map(|(send, line)| (send.from, line))
                .chain(keys)
                .filter(|(from, _)| *from != track)
                .map(|(_, line)| *line)
                .collect()
        };
        assert_eq!(from_others(&before), from_others(&after), "seed {seed}");
        let others = |plan: &Compensation| -> Vec<(u128, u64)> {
            plan.stage_inputs
                .iter()
                .copied()
                .filter(|(device, _)| !own.contains(device))
                .collect()
        };
        assert_eq!(others(&before), others(&after), "seed {seed}");
    }
}
