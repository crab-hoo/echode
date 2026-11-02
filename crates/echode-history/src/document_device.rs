//! Device JSON layout without a second list of model fields.

use std::io::{self, Write};

use echode_model::Device;
use serde::Serialize;

pub(super) fn render(device: &Device) -> String {
    let mut buffer = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut buffer, DeviceFormatter::default());
    device
        .serialize(&mut serializer)
        .expect("device values are finite JSON");
    String::from_utf8(buffer).expect("device JSON is UTF-8")
}

/// Only the parameter array is multiline. Pins and parameter objects remain
/// compact; escaping and key order still belong to Serde, not string splitting.
#[derive(Default)]
struct DeviceFormatter {
    in_key: bool,
    parameters_next: bool,
    array_depth: usize,
    parameters_depth: Option<usize>,
    parameters_empty: bool,
}

impl serde_json::ser::Formatter for DeviceFormatter {
    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        super::PackageFormatter.write_f32(writer, value)
    }

    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.in_key = true;
        if first {
            Ok(())
        } else {
            writer.write_all(b",")
        }
    }

    fn end_object_key<W: ?Sized + Write>(&mut self, _writer: &mut W) -> io::Result<()> {
        self.in_key = false;
        Ok(())
    }

    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        if self.in_key {
            self.parameters_next = fragment == "parameters";
        }
        writer.write_all(fragment.as_bytes())
    }

    fn begin_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.array_depth += 1;
        if std::mem::take(&mut self.parameters_next) {
            self.parameters_depth = Some(self.array_depth);
            self.parameters_empty = true;
            writer.write_all(b"[\n")
        } else {
            writer.write_all(b"[")
        }
    }

    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if self.parameters_depth == Some(self.array_depth) {
            self.parameters_empty = false;
            writer.write_all(if first { b"      " } else { b",\n      " })
        } else if first {
            Ok(())
        } else {
            writer.write_all(b",")
        }
    }

    fn end_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        let multiline = self.parameters_depth == Some(self.array_depth);
        self.array_depth -= 1;
        if multiline {
            self.parameters_depth = None;
            writer.write_all(if self.parameters_empty {
                b"    ]"
            } else {
                b"\n    ]"
            })
        } else {
            writer.write_all(b"]")
        }
    }
}
