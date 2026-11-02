//! Track header layout without a second list of scalar model fields.

use std::io::{self, Write};

use echode_model::{Id, Track, TrackMetadata};
use serde::Serialize;

pub(super) fn render_header(track: &Track, schema: u32) -> String {
    #[derive(Serialize)]
    struct Header {
        schema: u32,
        id: Id,
        #[serde(flatten)]
        metadata: TrackMetadata,
    }

    let mut buffer = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut buffer, HeaderFormatter::default());
    Header {
        schema,
        id: track.id,
        metadata: TrackMetadata::from(track),
    }
    .serialize(&mut serializer)
    .expect("track metadata is JSON");
    let mut text = String::from_utf8(buffer).expect("track metadata is UTF-8");
    // Continue this root object with separately formatted/resolved child arrays.
    assert!(text.ends_with("\n}"));
    text.truncate(text.len() - 2);
    text.push_str(",\n");
    text
}

/// Only root fields are multiline. Escaping, scalar spelling and key order are
/// Serde's responsibility; any nested values retain compact JSON formatting.
#[derive(Default)]
struct HeaderFormatter {
    depth: usize,
}

impl serde_json::ser::Formatter for HeaderFormatter {
    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        super::PackageFormatter.write_f32(writer, value)
    }

    fn begin_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.depth += 1;
        writer.write_all(b"{")
    }

    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if self.depth == 1 {
            writer.write_all(if first { b"\n  " } else { b",\n  " })
        } else if first {
            Ok(())
        } else {
            writer.write_all(b",")
        }
    }

    fn end_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.depth -= 1;
        writer.write_all(if self.depth == 0 { b"\n}" } else { b"}" })
    }
}
