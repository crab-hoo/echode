use super::{MEMBERS, MpeZone, ZoneConfigs};
use clack_host::events::io::InputEvents;
use clack_host::events::spaces::CoreEventSpace;

/// A deterministic generator: the sequences below are this test's input only.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound as u64) as usize
    }
}

#[test]
fn the_configuration_selects_a_fifteen_member_lower_zone_on_each_manager() {
    let zones = [MpeZone::new(3), MpeZone::new(9)];
    let configs = ZoneConfigs(&zones);
    let events = InputEvents::from_buffer(&configs);
    let messages: Vec<_> = events
        .iter()
        .map(|event| match event.as_core_event() {
            Some(CoreEventSpace::Midi(midi)) => {
                (event.header().time(), midi.port_index(), midi.data())
            }
            _ => panic!("MIDI 1.0 only"),
        })
        .collect();
    let zone = |port| {
        [
            (0, port, [0xB0, 100, 6]),
            (0, port, [0xB0, 101, 0]),
            (0, port, [0xB0, 6, 15]),
            (0, port, [0xB1, 100, 0]),
            (0, port, [0xB1, 101, 0]),
            (0, port, [0xB1, 6, 48]),
        ]
    };
    assert_eq!(messages, [zone(3), zone(9)].concat());
    assert!(events.get(12).is_none());
}

#[test]
fn fifteen_notes_own_distinct_members_and_later_ones_share_the_least_occupied() {
    let mut zone = MpeZone::new(0);
    let owned: Vec<u8> = (0..MEMBERS as u32).map(|id| zone.start(id, 60)).collect();
    assert_eq!(owned, (1..=15).collect::<Vec<u8>>());
    // A repeated note-on stays on its channel and is not counted twice.
    assert_eq!(zone.start(4, 60), 5);
    // Past fifteen, notes share channels from the least recently assigned.
    assert_eq!(zone.start(100, 61), 1);
    assert_eq!(zone.start(101, 61), 2);
    assert_eq!(zone.end(4, 60), Some(5));
    assert_eq!(zone.end(4, 60), None);
    // A free channel wins over sharing, wherever the cursor stands.
    assert_eq!(zone.start(102, 62), 5);
    assert_eq!(zone.end(100, 61), Some(1));
    assert_eq!(zone.end(101, 61), Some(2));
    assert_eq!(zone.end(100, 61), None);
    // Shared notes are found by key; an unknown note of another key ends nothing.
    assert_eq!(zone.end(999, 61), None);
    zone.clear();
    assert_eq!(zone.end(0, 60), None);
    assert_eq!(zone.end(102, 62), None);
    assert_eq!(zone.start(7, 60), 1);
}

/// Random starts, ends and resets, replayed into a receiver that counts each
/// channel's sounding keys. Every note-off reaches a sounding voice of its key,
/// an owner's reaches its own channel, at most fifteen held notes never share
/// a channel, and ending every held note leaves the receiver silent.
#[test]
fn every_note_off_reaches_its_voice_under_random_overlaps_and_resets() {
    for seed in 0..64 {
        let mut random = Lcg(seed);
        let mut zone = MpeZone::new(0);
        let mut voices = [[0u32; 128]; 16];
        let mut held: Vec<(u32, u8, Option<u8>)> = Vec::new();
        let mut next_id = 0u32;
        for _ in 0..2_000 {
            match random.below(10) {
                0..=4 => {
                    // A few keys force same-key overlaps across channels.
                    let key = 60 + random.below(4) as u8;
                    let id = next_id;
                    next_id += 1;
                    let channel = zone.start(id, key);
                    assert!((1..=15).contains(&channel), "seed {seed}");
                    let row = &mut voices[usize::from(channel)];
                    let free = row.iter().all(|count| *count == 0);
                    if held.len() < MEMBERS {
                        assert!(free, "seed {seed}: {} held notes shared", held.len());
                    }
                    row[usize::from(key)] += 1;
                    held.push((id, key, free.then_some(channel)));
                }
                5..=8 if !held.is_empty() => {
                    let (id, key, owned) = held.swap_remove(random.below(held.len()));
                    let channel = zone.end(id, key).expect("a held note ends");
                    if let Some(owned) = owned {
                        assert_eq!(channel, owned, "seed {seed}: owner moved");
                    }
                    let count = &mut voices[usize::from(channel)][usize::from(key)];
                    assert!(*count > 0, "seed {seed}: no voice of key {key}");
                    *count -= 1;
                }
                9 if random.below(20) == 0 => {
                    zone.clear();
                    voices = [[0; 128]; 16];
                    held.clear();
                }
                // Owners are found by id and shared notes by key: a key
                // never played ends nothing.
                _ => assert_eq!(zone.end(u32::MAX, 100), None),
            }
        }
        for (id, key, _) in held.drain(..) {
            let channel = zone.end(id, key).expect("a held note ends");
            voices[usize::from(channel)][usize::from(key)] -= 1;
        }
        assert!(
            voices.iter().flatten().all(|count| *count == 0),
            "seed {seed}"
        );
        // A silent zone gives the next note the least recently assigned channel.
        let expected = zone.next as u8 + 1;
        assert_eq!(zone.start(next_id, 60), expected);
    }
}
