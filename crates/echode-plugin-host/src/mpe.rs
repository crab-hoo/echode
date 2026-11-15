//! Member channels for MPE note input ports (MPE v1.1 Lower Zone).
//!
//! The helper configures fifteen member channels (MIDI channels 2 to 16) on
//! every MPE-only port and plays each project note on one of them. A note that
//! finds a free channel owns it until its note-off, so up to fifteen held notes
//! never share a channel, even at the same key. Past fifteen, a note shares the
//! least occupied channel, as MPE allows.
//!
//! Every note-on sent is counted exactly once, as an owner or a shared key, and
//! every note-off sent removes exactly one count. A note-off therefore reaches
//! the channel of its note-on, and one this zone never started is not sent.
//! Storage is fixed per port, built on main, and only cleared on audio.

use clack_host::events::event_types::MidiEvent;
use clack_host::events::io::InputEventBuffer;
use clack_host::events::{Event as _, UnknownEvent};

/// Member channels of a Lower Zone that uses every channel after the manager.
pub(crate) const MEMBERS: usize = 15;
const KEYS: usize = 128;

pub struct MpeZone {
    port: u16,
    /// The MPE Configuration Message for this port, sent at time zero.
    configuration: [MidiEvent; 3],
    /// The note id that took each member channel while it was free.
    owners: [Option<u32>; MEMBERS],
    /// Notes placed on each channel after all fifteen were taken, by key.
    shared: [[u32; KEYS]; MEMBERS],
    /// The sum of each channel's `shared` row.
    shared_total: [u32; MEMBERS],
    /// The member tried first for the next note, after the last one assigned,
    /// so a just-released channel's release is the last to be reused.
    next: usize,
}

impl MpeZone {
    /// RPN 6 on the manager channel (MIDI channel 1) selects fifteen member
    /// channels, in the order JUCE's `MPEMessages::setLowerZone` sends it.
    pub fn new(port: u16) -> Self {
        let members = MEMBERS as u8;
        Self {
            port,
            configuration: [
                MidiEvent::new(0, port, [0xB0, 100, 6]),
                MidiEvent::new(0, port, [0xB0, 101, 0]),
                MidiEvent::new(0, port, [0xB0, 6, members]),
            ],
            owners: [None; MEMBERS],
            shared: [[0; KEYS]; MEMBERS],
            shared_total: [0; MEMBERS],
            next: 0,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// No note sounds: after a reset or before a new processing run.
    pub fn clear(&mut self) {
        self.owners = [None; MEMBERS];
        for (row, total) in self.shared.iter_mut().zip(&mut self.shared_total) {
            if *total != 0 {
                *row = [0; KEYS];
                *total = 0;
            }
        }
        self.next = 0;
    }

    /// The channel nibble (1 to 15: MIDI channels 2 to 16) of a note-on. A
    /// repeated note-on of a note that owns a channel goes there again and is
    /// not counted twice.
    pub(crate) fn start(&mut self, note_id: u32, key: u8) -> u8 {
        if let Some(member) = self.owner(note_id) {
            return channel(member);
        }
        let order = (0..MEMBERS).map(|offset| (self.next + offset) % MEMBERS);
        let free = order.clone().find(|&member| {
            self.owners.get(member).is_some_and(Option::is_none)
                && self.shared_total.get(member) == Some(&0)
        });
        let member = if let Some(member) = free {
            if let Some(owner) = self.owners.get_mut(member) {
                *owner = Some(note_id);
            }
            member
        } else {
            // Every channel has an owner or shared notes. `min_by_key` keeps
            // the first of equals, the least recently assigned.
            let member = order
                .min_by_key(|&member| self.occupancy(member))
                .unwrap_or(0);
            // A count cannot reach u32::MAX: each held note took an event.
            if let Some(count) = self
                .shared
                .get_mut(member)
                .and_then(|row| row.get_mut(usize::from(key)))
            {
                *count = count.saturating_add(1);
            }
            if let Some(total) = self.shared_total.get_mut(member) {
                *total = total.saturating_add(1);
            }
            member
        };
        self.next = (member + 1) % MEMBERS;
        channel(member)
    }

    /// The channel nibble of the sounding note a note-off or choke ends, or
    /// `None` when this zone started no such note.
    pub(crate) fn end(&mut self, note_id: u32, key: u8) -> Option<u8> {
        if let Some(member) = self.owner(note_id) {
            if let Some(owner) = self.owners.get_mut(member) {
                *owner = None;
            }
            return Some(channel(member));
        }
        let key = usize::from(key);
        let member = self
            .shared
            .iter()
            .position(|row| row.get(key).is_some_and(|count| *count > 0))?;
        if let Some(count) = self.shared.get_mut(member).and_then(|row| row.get_mut(key)) {
            *count -= 1;
        }
        if let Some(total) = self.shared_total.get_mut(member) {
            *total -= 1;
        }
        Some(channel(member))
    }

    fn occupancy(&self, member: usize) -> u32 {
        let owned = self.owners.get(member).is_some_and(Option::is_some);
        let shared = self.shared_total.get(member).copied().unwrap_or(u32::MAX);
        shared.saturating_add(u32::from(owned))
    }

    fn owner(&self, note_id: u32) -> Option<usize> {
        self.owners.iter().position(|owner| *owner == Some(note_id))
    }
}

/// Every zone's configuration in port order, read in place on the audio
/// thread: the list cannot disagree with the zones that use it.
pub(crate) struct ZoneConfigs<'a>(pub(crate) &'a [MpeZone]);

impl InputEventBuffer for ZoneConfigs<'_> {
    fn len(&self) -> u32 {
        // Ports are addressed by u16, so this product stays far below u32::MAX.
        u32::try_from(self.0.len().saturating_mul(3)).unwrap_or(u32::MAX)
    }

    fn get(&self, index: u32) -> Option<&UnknownEvent> {
        let index = index as usize;
        let zone = self.0.get(index / 3)?;
        // The slice's own `get`, not the array's `InputEventBuffer::get`.
        <[MidiEvent]>::get(&zone.configuration, index % 3).map(|event| event.as_unknown())
    }
}

/// Member `0` is nibble 1 (MIDI channel 2), the first after the manager.
fn channel(member: usize) -> u8 {
    (member % MEMBERS) as u8 + 1
}

#[cfg(test)]
#[path = "mpe_tests.rs"]
mod tests;
