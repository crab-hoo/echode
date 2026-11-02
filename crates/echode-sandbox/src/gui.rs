//! Plugin windows and the changes a plugin makes on its own.
//!
//! The helper owns the window. The parent asks it to show or close the window
//! and hears back when the user closed it. Values the plugin changes itself,
//! for example from its window, arrive as a relay notice in the order the
//! plugin reported them, with the values read back afterwards.

use std::io;

use crate::params::{Cursor, ParamValue, invalid, len32, read_values_from, write_values_into};

/// What showing a plugin window did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuiReport {
    /// This request created the window. False when it was open and was raised.
    pub created: bool,
    /// The window is on screen. False while it waits for Echode to be active.
    pub visible: bool,
    /// The window is the key window. Showing it never makes it key.
    pub key: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamEventKind {
    /// The plugin changed the value. Read-only parameters are not relayed.
    Value,
    /// The user started adjusting the parameter in the plugin.
    Begin,
    /// The user finished adjusting the parameter in the plugin.
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamEvent {
    pub id: u32,
    pub kind: ParamEventKind,
}

/// Changes a plugin made on its own, in the order it reported them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParamRelay {
    pub events: Vec<ParamEvent>,
    /// The current value and text of each parameter with a value event.
    pub values: Vec<ParamValue>,
    /// The plugin asked the host to read its parameter descriptions again.
    pub rescan: bool,
    /// Some events were dropped. Values and gestures may be incomplete, so a
    /// reader reads the list again and treats the change as finished.
    pub lost: bool,
}

impl ParamRelay {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.values.is_empty() && !self.rescan && !self.lost
    }

    /// Appends a later relay. A newer value replaces an older one for the same id.
    pub fn merge(&mut self, later: Self) {
        self.events.extend(later.events);
        for value in later.values {
            match self.values.iter_mut().find(|known| known.id == value.id) {
                Some(known) => *known = value,
                None => self.values.push(value),
            }
        }
        self.rescan |= later.rescan;
        self.lost |= later.lost;
    }
}

/// What one helper reported since the last read, for the device view and history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginActivity {
    /// Global DAW commands received while the plugin window had focus.
    pub commands: Vec<crate::PluginCommand>,
    pub relay: ParamRelay,
    /// The plugin marked its state changed while its window was open.
    pub dirty: bool,
    /// The user closed the plugin window.
    pub closed: bool,
    /// The window was open when the relay or the dirty notice arrived. Only
    /// then does a change without a gesture count as the user's edit (ADR 0132).
    pub window_open: bool,
    /// Latest confirmed window lifetime, independent of visibility and of
    /// whether an earlier relay arrived while the window was open.
    pub window_state: Option<bool>,
}

impl PluginActivity {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
            && self.relay.is_empty()
            && !self.dirty
            && !self.closed
            && self.window_state.is_none()
    }

    pub fn merge(&mut self, later: Self) {
        self.commands.extend(later.commands);
        self.relay.merge(later.relay);
        self.dirty |= later.dirty;
        self.closed |= later.closed;
        self.window_open |= later.window_open;
        self.window_state = later.window_state.or(self.window_state);
    }
}

pub(crate) fn write_report(report: GuiReport) -> Vec<u8> {
    vec![
        u8::from(report.created),
        u8::from(report.visible),
        u8::from(report.key),
    ]
}

pub(crate) fn read_report(body: &[u8]) -> io::Result<GuiReport> {
    let [created, visible, key] = body else {
        return Err(invalid("window report is the wrong size"));
    };
    let flag = |byte: &u8| match byte {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid("window report flag is not 0 or 1")),
    };
    Ok(GuiReport {
        created: flag(created)?,
        visible: flag(visible)?,
        key: flag(key)?,
    })
}

const RESCAN: u8 = 1;
const LOST: u8 = 2;

pub(crate) fn write_relay(relay: &ParamRelay) -> Vec<u8> {
    let mut body = Vec::with_capacity(5 + relay.events.len() * 5);
    let mut flags = 0;
    if relay.rescan {
        flags |= RESCAN;
    }
    if relay.lost {
        flags |= LOST;
    }
    body.push(flags);
    body.extend_from_slice(&len32(relay.events.len()).to_le_bytes());
    for event in &relay.events {
        body.extend_from_slice(&event.id.to_le_bytes());
        body.push(match event.kind {
            ParamEventKind::Value => 0,
            ParamEventKind::Begin => 1,
            ParamEventKind::End => 2,
        });
    }
    write_values_into(&mut body, &relay.values);
    body
}

pub(crate) fn read_relay(body: &[u8]) -> io::Result<ParamRelay> {
    let mut cursor = Cursor::new(body);
    let flags = cursor.u8()?;
    if flags & !(RESCAN | LOST) != 0 {
        return Err(invalid("relay flags are unknown"));
    }
    let count = cursor.u32()? as usize;
    // Each event is 5 bytes, so a corrupt count cannot reserve more than the message holds.
    let mut events = Vec::with_capacity(count.min(body.len() / 5));
    for _ in 0..count {
        let id = cursor.u32()?;
        let kind = match cursor.u8()? {
            0 => ParamEventKind::Value,
            1 => ParamEventKind::Begin,
            2 => ParamEventKind::End,
            _ => return Err(invalid("relay event kind is unknown")),
        };
        events.push(ParamEvent { id, kind });
    }
    let values = read_values_from(&mut cursor)?;
    cursor.finish()?;
    Ok(ParamRelay {
        events,
        values,
        rescan: flags & RESCAN != 0,
        lost: flags & LOST != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay() -> ParamRelay {
        ParamRelay {
            events: vec![
                ParamEvent {
                    id: 1,
                    kind: ParamEventKind::Begin,
                },
                ParamEvent {
                    id: 1,
                    kind: ParamEventKind::Value,
                },
                ParamEvent {
                    id: 1,
                    kind: ParamEventKind::End,
                },
            ],
            values: vec![ParamValue {
                id: 1,
                value: 1.5,
                text: "1.50x".into(),
            }],
            rescan: true,
            lost: false,
        }
    }

    #[test]
    fn relays_round_trip_and_reject_damage() {
        let body = write_relay(&relay());
        assert_eq!(read_relay(&body).unwrap(), relay());
        assert!(read_relay(&body[..body.len() - 1]).is_err());
        let mut unknown = body.clone();
        unknown[0] = 4;
        assert!(read_relay(&unknown).is_err());
        let mut kind = body.clone();
        kind[9] = 3;
        assert!(read_relay(&kind).is_err());
        // A huge count with no events must fail without reserving that many.
        let mut huge = vec![0];
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_relay(&huge).is_err());
    }

    #[test]
    fn reports_round_trip_and_reject_damage() {
        let report = GuiReport {
            created: true,
            visible: false,
            key: false,
        };
        assert_eq!(read_report(&write_report(report)).unwrap(), report);
        assert!(read_report(&[1, 1]).is_err());
        assert!(read_report(&[1, 2, 0]).is_err());
    }

    #[test]
    fn merging_keeps_order_and_the_newest_value() {
        let mut first = PluginActivity {
            relay: relay(),
            ..PluginActivity::default()
        };
        first.merge(PluginActivity {
            relay: ParamRelay {
                events: vec![ParamEvent {
                    id: 1,
                    kind: ParamEventKind::Value,
                }],
                values: vec![ParamValue {
                    id: 1,
                    value: 0.5,
                    text: "0.50x".into(),
                }],
                rescan: false,
                lost: true,
            },
            dirty: true,
            closed: false,
            window_open: true,
            commands: vec![crate::PluginCommand::Playback],
            window_state: Some(true),
        });
        assert_eq!(first.relay.events.len(), 4);
        assert_eq!(first.relay.values.len(), 1);
        assert_eq!(first.relay.values[0].value, 0.5);
        assert!(first.relay.rescan && first.relay.lost && first.dirty && !first.closed);
        assert!(first.window_open);
        assert_eq!(first.commands, [crate::PluginCommand::Playback]);
        assert_eq!(first.window_state, Some(true));
        first.merge(PluginActivity {
            window_state: Some(false),
            ..PluginActivity::default()
        });
        first.merge(PluginActivity::default());
        assert_eq!(first.window_state, Some(false));
        assert!(
            first.window_open,
            "the earlier edit's context is independent"
        );
        assert!(PluginActivity::default().is_empty());
    }
}
