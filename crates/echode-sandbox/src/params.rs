//! CLAP parameter descriptions and value changes on the control socket.
//!
//! The helper reads descriptions on the plugin's main thread. Values stay in
//! the plugin's own units between `min` and `max`; nothing is normalized here.
//! The count is not capped. The 64 MiB message bound still applies.

use std::io::{self, ErrorKind};

/// `CLAP_PARAM_IS_STEPPED`. The flag values are CLAP's, so the parent does not link `clap-sys`.
pub const PARAM_STEPPED: u32 = 1 << 0;
pub const PARAM_PERIODIC: u32 = 1 << 1;
pub const PARAM_HIDDEN: u32 = 1 << 2;
pub const PARAM_READONLY: u32 = 1 << 3;
pub const PARAM_BYPASS: u32 = 1 << 4;
pub const PARAM_AUTOMATABLE: u32 = 1 << 5;
pub const PARAM_ENUM: u32 = 1 << 16;

/// One parameter as the plugin describes it, with its current value and text.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamInfo {
    pub id: u32,
    pub flags: u32,
    pub name: String,
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub value: f64,
    /// The plugin's `value_to_text`. Empty when the plugin gives none.
    pub text: String,
}

impl ParamInfo {
    pub fn stepped(&self) -> bool {
        self.flags & PARAM_STEPPED != 0
    }

    pub fn hidden(&self) -> bool {
        self.flags & PARAM_HIDDEN != 0
    }

    pub fn read_only(&self) -> bool {
        self.flags & PARAM_READONLY != 0
    }

    /// Range and value a host can show and edit. A plugin that reports a
    /// non-finite or inverted range is skipped by the helper, not clamped.
    pub fn usable(&self) -> bool {
        self.min.is_finite()
            && self.max.is_finite()
            && self.default.is_finite()
            && self.value.is_finite()
            && self.min <= self.max
    }
}

/// A new value for one parameter. The helper applies a batch in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamChange {
    pub id: u32,
    pub value: f64,
}

/// A parameter's value and text, read back after the plugin applied a change.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamValue {
    pub id: u32,
    pub value: f64,
    pub text: String,
}

pub(crate) fn write_infos(infos: &[ParamInfo]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&len32(infos.len()).to_le_bytes());
    for info in infos {
        body.extend_from_slice(&info.id.to_le_bytes());
        body.extend_from_slice(&info.flags.to_le_bytes());
        for value in [info.min, info.max, info.default, info.value] {
            body.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        for text in [&info.name, &info.module, &info.text] {
            write_text(&mut body, text);
        }
    }
    body
}

pub(crate) fn read_infos(body: &[u8]) -> io::Result<Vec<ParamInfo>> {
    let mut cursor = Cursor { body, at: 0 };
    let count = cursor.u32()? as usize;
    // Each entry needs at least 52 bytes, so a corrupt count cannot reserve
    // more than the message already holds.
    let mut infos = Vec::with_capacity(count.min(body.len() / 52));
    for _ in 0..count {
        let id = cursor.u32()?;
        let flags = cursor.u32()?;
        let min = cursor.f64()?;
        let max = cursor.f64()?;
        let default = cursor.f64()?;
        let value = cursor.f64()?;
        let name = cursor.text()?;
        let module = cursor.text()?;
        let text = cursor.text()?;
        infos.push(ParamInfo {
            id,
            flags,
            name,
            module,
            min,
            max,
            default,
            value,
            text,
        });
    }
    cursor.finish()?;
    Ok(infos)
}

pub(crate) fn write_values(values: &[ParamValue]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&len32(values.len()).to_le_bytes());
    for value in values {
        body.extend_from_slice(&value.id.to_le_bytes());
        body.extend_from_slice(&value.value.to_bits().to_le_bytes());
        write_text(&mut body, &value.text);
    }
    body
}

pub(crate) fn read_values(body: &[u8]) -> io::Result<Vec<ParamValue>> {
    let mut cursor = Cursor { body, at: 0 };
    let count = cursor.u32()? as usize;
    let mut values = Vec::with_capacity(count.min(body.len() / 16));
    for _ in 0..count {
        let id = cursor.u32()?;
        let value = cursor.f64()?;
        let text = cursor.text()?;
        values.push(ParamValue { id, value, text });
    }
    cursor.finish()?;
    Ok(values)
}

pub(crate) fn write_changes(changes: &[ParamChange]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + changes.len() * 12);
    body.extend_from_slice(&len32(changes.len()).to_le_bytes());
    for change in changes {
        body.extend_from_slice(&change.id.to_le_bytes());
        body.extend_from_slice(&change.value.to_bits().to_le_bytes());
    }
    body
}

pub(crate) fn read_changes(body: &[u8]) -> io::Result<Vec<ParamChange>> {
    let mut cursor = Cursor { body, at: 0 };
    let count = cursor.u32()? as usize;
    let mut changes = Vec::with_capacity(count.min(body.len() / 12));
    for _ in 0..count {
        let id = cursor.u32()?;
        let value = cursor.f64()?;
        if !value.is_finite() {
            return Err(invalid("parameter value is not finite"));
        }
        changes.push(ParamChange { id, value });
    }
    cursor.finish()?;
    Ok(changes)
}

fn len32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

fn write_text(body: &mut Vec<u8>, text: &str) {
    body.extend_from_slice(&len32(text.len()).to_le_bytes());
    body.extend_from_slice(text.as_bytes());
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

struct Cursor<'a> {
    body: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, len: usize) -> io::Result<&[u8]> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.body.len())
            .ok_or_else(|| invalid("parameter message is truncated"))?;
        let bytes = &self.body[self.at..end];
        self.at = end;
        Ok(bytes)
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn f64(&mut self) -> io::Result<f64> {
        Ok(f64::from_bits(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        )))
    }

    fn text(&mut self) -> io::Result<String> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec())
            .map_err(|_| invalid("parameter text is not UTF-8"))
    }

    fn finish(&self) -> io::Result<()> {
        if self.at == self.body.len() {
            Ok(())
        } else {
            Err(invalid("parameter message has trailing bytes"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain() -> ParamInfo {
        ParamInfo {
            id: 7,
            flags: PARAM_AUTOMATABLE | PARAM_STEPPED,
            name: "Gain".into(),
            module: "Main/Out".into(),
            min: 0.0,
            max: 2.0,
            default: 1.0,
            value: 0.5,
            text: "0.50x".into(),
        }
    }

    #[test]
    fn descriptions_round_trip_and_reject_truncation() {
        let infos = vec![
            gain(),
            ParamInfo {
                id: 9,
                text: String::new(),
                ..gain()
            },
        ];
        let body = write_infos(&infos);
        assert_eq!(read_infos(&body).unwrap(), infos);
        assert!(read_infos(&body[..body.len() - 1]).is_err());
        let mut longer = body.clone();
        longer.push(0);
        assert!(read_infos(&longer).is_err());
        // A huge count with no entries must fail without reserving that many.
        assert!(read_infos(&u32::MAX.to_le_bytes()).is_err());
        assert!(read_infos(&0u32.to_le_bytes()).unwrap().is_empty());
    }

    #[test]
    fn changes_round_trip_and_reject_non_finite_values() {
        let changes = [
            ParamChange { id: 1, value: 0.25 },
            ParamChange { id: 1, value: 2.0 },
        ];
        assert_eq!(read_changes(&write_changes(&changes)).unwrap(), changes);
        for value in [f64::NAN, f64::INFINITY] {
            assert!(read_changes(&write_changes(&[ParamChange { id: 1, value }])).is_err());
        }
    }

    #[test]
    fn applied_values_round_trip() {
        let values = vec![ParamValue {
            id: 4,
            value: -1.5,
            text: "-1.5 dB".into(),
        }];
        assert_eq!(read_values(&write_values(&values)).unwrap(), values);
        let body = write_values(&values);
        assert!(read_values(&body[..body.len() - 2]).is_err());
    }

    #[test]
    fn usable_ranges_are_finite_and_ordered() {
        assert!(gain().usable());
        assert!(!ParamInfo { min: 3.0, ..gain() }.usable());
        assert!(
            !ParamInfo {
                max: f64::INFINITY,
                ..gain()
            }
            .usable()
        );
        assert!(
            !ParamInfo {
                value: f64::NAN,
                ..gain()
            }
            .usable()
        );
        assert!(gain().stepped() && !gain().hidden() && !gain().read_only());
    }
}
