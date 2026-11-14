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
    }
}

fn delays(source: u64, instrument: u64, output: u64) -> TrackDelays {
    TrackDelays {
        source,
        instrument,
        output,
    }
}

#[test]
fn top_level_tracks_wait_for_the_latest_one_at_master() {
    let chains = [[device(100)], [device(0)], [device(30)]];
    let flows: Vec<_> = chains.iter().map(|chain| flow(None, true, chain)).collect();
    let plan = compensate(&flows).unwrap();
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
    let plan = compensate(&flows).unwrap();
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
    let plan = compensate(&[flow(None, true, &slow), flow(Some(0), false, &child)]).unwrap();
    assert_eq!(plan.tracks, [delays(64, 0, 0), delays(0, 0, 48)]);
    assert_eq!(plan.latency, 72);
    // The child is later than the instrument: the instrument waits.
    let fast = [instrument(4), device(8)];
    let plan = compensate(&[flow(None, true, &fast), flow(Some(0), false, &child)]).unwrap();
    assert_eq!(plan.tracks, [delays(16, 12, 0), delays(0, 0, 0)]);
    assert_eq!(plan.latency, 24);
}

#[test]
fn a_line_exists_only_where_audio_flows() {
    // No placed audio and no instrument: nothing to delay at the input.
    let child = [device(16)];
    let plan = compensate(&[flow(None, false, &[]), flow(Some(0), false, &child)]).unwrap();
    assert_eq!(plan.tracks, [delays(0, 0, 0), delays(0, 0, 0)]);
    // A track that plays nothing still joins master in time.
    let plan = compensate(&[flow(None, false, &[]), flow(None, false, &child)]).unwrap();
    assert_eq!(plan.tracks, [delays(0, 0, 16), delays(0, 0, 0)]);
}

#[test]
fn a_bypassed_device_keeps_its_place_and_delays_its_own_output() {
    let bypassed = [DeviceLatency {
        bypassed: true,
        ..device(100)
    }];
    let plan = compensate(&[flow(None, true, &bypassed), flow(None, true, &[])]).unwrap();
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
    let plan = compensate(&[flow(None, true, &silent)]).unwrap();
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
    assert_eq!(compensate(&flows).unwrap().tail, 123);
    let chains = [tailed(u32::MAX), tailed(1), tailed(1), tailed(0)];
    let flows: Vec<_> = chains
        .iter()
        .zip(parents)
        .map(|(chain, parent)| flow(parent, true, chain))
        .collect();
    assert_eq!(compensate(&flows).unwrap().tail, u32::MAX);
    // A child's audio does not pass its parent's instrument.
    let parent = [
        DeviceLatency {
            tail: 1_000,
            ..instrument(0)
        },
        tailed(5)[0],
    ];
    let child = tailed(40);
    let plan = compensate(&[flow(None, false, &parent), flow(Some(0), true, &child)]).unwrap();
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
    let plan = compensate(&flows).unwrap();
    assert_eq!(plan.latency, depth as u64);
    assert!(plan.tracks.iter().all(|track| track.output == 0));
    assert_eq!(plan.tracks[0].source, depth as u64 - 1);
}

#[test]
fn latencies_past_32_bits_add_up_and_a_parent_after_its_child_is_refused() {
    // Each device reports 32 bits; two in a row already pass that.
    let huge = vec![
        DeviceLatency {
            bypassed: true,
            ..device(u32::MAX)
        };
        2
    ];
    let plan = compensate(&[flow(None, true, &huge)]).unwrap();
    assert_eq!(plan.latency, 2 * u64::from(u32::MAX));
    assert!(compensate(&[flow(Some(1), true, &[]), flow(None, true, &[])]).is_err());
    assert!(compensate(&[flow(Some(0), true, &[])]).is_err());
}

/// What the callback does with one track's lines, as plain delays: placed
/// audio through `source`, the first instrument through its latency and
/// `instrument`, the children as they arrive, then every later device, and
/// `output` before the track joins its parent. A bypassed device passes its
/// audio at once. Returns when each impulse from the timeline reaches master.
fn arrivals(flows: &[TrackFlow<'_>], plan: &Compensation) -> Vec<u64> {
    // Children are after their parents, so walk backwards to have each
    // child's arrivals before its parent mixes them.
    let mut joining: Vec<Vec<u64>> = vec![Vec::new(); flows.len()];
    let mut master = vec![plan.clicks];
    for (position, (track, delays)) in flows.iter().zip(&plan.tracks).enumerate().rev() {
        let mut at_input = std::mem::take(&mut joining[position]);
        if track.clips {
            at_input.push(delays.source);
        }
        let mut devices = track.devices;
        if let Some((first, later)) = devices.split_first()
            && first.instrument
        {
            if !first.bypassed {
                at_input.push(u64::from(first.latency) + delays.instrument);
            }
            devices = later;
        }
        let chain: u64 = devices
            .iter()
            .filter(|device| !device.bypassed)
            .map(|device| u64::from(device.latency))
            .sum();
        let out = at_input
            .into_iter()
            .map(|arrival| arrival + chain + delays.output);
        match track.parent {
            Some(parent) => joining[parent].extend(out),
            None => master.extend(out),
        }
    }
    master
}

#[test]
fn random_folders_reach_master_together_through_the_callbacks_lines() {
    for seed in 1..=300u64 {
        let mut random = Random(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93));
        let count = 1 + random.below(12) as usize;
        let chains: Vec<Vec<DeviceLatency>> = (0..count)
            .map(|_| {
                (0..random.below(4))
                    .map(|index| DeviceLatency {
                        device: 0,
                        latency: random.latency() as u32,
                        tail: 0,
                        instrument: index == 0 && random.below(2) == 0,
                        bypassed: random.below(4) == 0,
                    })
                    .collect()
            })
            .collect();
        let flows: Vec<_> = chains
            .iter()
            .enumerate()
            .map(|(position, chain)| TrackFlow {
                parent: (position != 0 && random.below(3) != 0)
                    .then(|| random.below(position as u64) as usize),
                clips: random.below(3) != 0,
                devices: chain,
            })
            .collect();
        let plan = compensate(&flows).unwrap();
        for arrival in arrivals(&flows, &plan) {
            assert_eq!(arrival, plan.latency, "seed {seed}: {flows:?}");
        }
    }
}
