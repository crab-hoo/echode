//! One JSON object per undo step.
//!
//! The same journal always produces the same bytes. Ids are hyphenated UUID
//! text and musical times are integers, so a later diff can show the edit.

use serde::{Deserialize, Serialize};

use echode_model::{
    AudioRef, Change, ChangeSet, ClipKind, Field, Id, Meter, ObjectKey, ObjectKind, Property,
    Tempo, Tick, Value,
};

use crate::action::{Action, ActionError};

/// One undo step. `changes` is absent on journal lines written before changesets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    pub actions: Vec<Action>,
    pub changes: Option<ChangeSet>,
}

/// A durable journal line. `undo` stores the changeset it undoes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalOp {
    Edit,
    Undo,
    Redo,
}

/// One appended journal record. The changeset is enough to replay the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalLine {
    pub seq: u64,
    pub op: JournalOp,
    pub actions: Vec<Action>,
    pub changes: ChangeSet,
}

/// Writes the journal. An empty journal is an empty string.
pub fn encode(journal: &[JournalEntry]) -> String {
    let mut out = String::new();
    for group in journal {
        debug_assert!(!group.actions.is_empty());
        let record = GroupRecord {
            actions: group.actions.clone(),
            changes: group.changes.as_ref().map(change_records),
        };
        let line = serde_json::to_string(&record).expect("action records have no failing fields");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Reads a journal written by [`encode`].
pub fn decode(text: &str) -> Result<Vec<JournalEntry>, ActionError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.lines().map(decode_group).collect()
}

fn decode_group(line: &str) -> Result<JournalEntry, ActionError> {
    let record: GroupRecord =
        serde_json::from_str(line).map_err(|_| ActionError::CorruptJournal)?;
    if record.actions.is_empty() {
        return Err(ActionError::CorruptJournal);
    }
    Ok(JournalEntry {
        actions: record.actions,
        changes: record
            .changes
            .map(|changes| parse_changes(&changes))
            .transpose()?,
    })
}

/// One journal line, without a trailing newline.
pub fn encode_line(line: &JournalLine) -> String {
    let record = LineRecord {
        seq: line.seq,
        kind: match line.op {
            JournalOp::Edit => "edit",
            JournalOp::Undo => "undo",
            JournalOp::Redo => "redo",
        }
        .to_string(),
        actions: line.actions.clone(),
        changes: change_records(&line.changes),
    };
    serde_json::to_string(&record).expect("journal lines have no failing fields")
}

/// Reads one journal line.
pub fn decode_line(text: &str) -> Result<JournalLine, ActionError> {
    let record: LineRecord = serde_json::from_str(text).map_err(|_| ActionError::CorruptJournal)?;
    let op = match record.kind.as_str() {
        "edit" => JournalOp::Edit,
        "undo" => JournalOp::Undo,
        "redo" => JournalOp::Redo,
        _ => return Err(ActionError::CorruptJournal),
    };
    Ok(JournalLine {
        seq: record.seq,
        op,
        actions: record.actions,
        changes: parse_changes(&record.changes)?,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupRecord {
    actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    changes: Option<Vec<ChangeRecord>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineRecord {
    seq: u64,
    kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    actions: Vec<Action>,
    changes: Vec<ChangeRecord>,
}

fn parse_id(text: &str) -> Result<Id, ActionError> {
    Id::parse(text).ok_or(ActionError::CorruptJournal)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum ChangeRecord {
    Create {
        kind: String,
        id: String,
        fields: Vec<FieldRecord>,
    },
    Delete {
        kind: String,
        id: String,
        fields: Vec<FieldRecord>,
    },
    Update {
        id: String,
        property: String,
        before: ValueRecord,
        after: ValueRecord,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldRecord {
    property: String,
    value: ValueRecord,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ValueRecord {
    Meter {
        numerator: u32,
        denominator: u32,
    },
    Tempo {
        numerator: u32,
        denominator: u32,
    },
    Tick {
        tick: i64,
    },
    Text {
        text: String,
    },
    Id {
        id: String,
    },
    Ids {
        ids: Vec<String>,
    },
    Parent {
        id: Option<String>,
    },
    Audio {
        hash: Option<String>,
        #[serde(default)]
        sample_rate: u32,
        #[serde(default)]
        channels: u16,
        #[serde(default)]
        frames: u64,
    },
    Kind {
        kind: String,
    },
    Byte {
        value: u8,
    },
    U16 {
        value: u16,
    },
    Gain {
        bits: u32,
    },
    Bits {
        bits: u32,
    },
    Flicks {
        value: i64,
    },
    ParamIds {
        ids: Vec<u32>,
    },
    OrderStep {
        index: u32,
        id: String,
        present: bool,
    },
}

fn change_records(changes: &ChangeSet) -> Vec<ChangeRecord> {
    changes.changes().iter().map(ChangeRecord::from).collect()
}

fn parse_changes(records: &[ChangeRecord]) -> Result<ChangeSet, ActionError> {
    let mut changes = ChangeSet::new();
    for record in records {
        changes.push(record.to_change()?);
    }
    Ok(changes)
}

impl From<&Change> for ChangeRecord {
    fn from(change: &Change) -> Self {
        match change {
            Change::Create { kind, id, fields } => Self::Create {
                kind: kind_name(*kind).to_string(),
                id: key_text(id),
                fields: fields.iter().map(FieldRecord::from).collect(),
            },
            Change::Delete { kind, id, fields } => Self::Delete {
                kind: kind_name(*kind).to_string(),
                id: key_text(id),
                fields: fields.iter().map(FieldRecord::from).collect(),
            },
            Change::Update {
                id,
                property,
                before,
                after,
            } => Self::Update {
                id: key_text(id),
                property: property_name(*property).to_string(),
                before: ValueRecord::from(before),
                after: ValueRecord::from(after),
            },
        }
    }
}

impl ChangeRecord {
    fn to_change(&self) -> Result<Change, ActionError> {
        Ok(match self {
            Self::Create { kind, id, fields } => Change::Create {
                kind: parse_kind(kind)?,
                id: parse_key(id)?,
                fields: parse_fields(fields)?,
            },
            Self::Delete { kind, id, fields } => Change::Delete {
                kind: parse_kind(kind)?,
                id: parse_key(id)?,
                fields: parse_fields(fields)?,
            },
            Self::Update {
                id,
                property,
                before,
                after,
            } => Change::Update {
                id: parse_key(id)?,
                property: parse_property(property)?,
                before: before.to_value()?,
                after: after.to_value()?,
            },
        })
    }
}

impl From<&Field> for FieldRecord {
    fn from(field: &Field) -> Self {
        Self {
            property: property_name(field.property).to_string(),
            value: ValueRecord::from(&field.value),
        }
    }
}

impl From<&Value> for ValueRecord {
    fn from(value: &Value) -> Self {
        match value {
            Value::Meter(meter) => Self::Meter {
                numerator: meter.numerator(),
                denominator: meter.denominator(),
            },
            Value::Tempo(tempo) => Self::Tempo {
                numerator: tempo.numerator(),
                denominator: tempo.denominator(),
            },
            Value::Tick(tick) => Self::Tick { tick: tick.get() },
            Value::Text(text) => Self::Text { text: text.clone() },
            Value::Id(id) => Self::Id { id: id.to_string() },
            Value::Ids(ids) => Self::Ids {
                ids: ids.iter().map(ToString::to_string).collect(),
            },
            Value::Parent(id) => Self::Parent {
                id: id.map(|id| id.to_string()),
            },
            Value::Audio(audio) => Self::Audio {
                hash: audio.as_ref().map(|audio| audio.hash.clone()),
                sample_rate: audio.as_ref().map(|audio| audio.sample_rate).unwrap_or(0),
                channels: audio.as_ref().map(|audio| audio.channels).unwrap_or(0),
                frames: audio.as_ref().map(|audio| audio.frames).unwrap_or(0),
            },
            Value::Kind(ClipKind::Auto) => Self::Kind {
                kind: "auto".to_string(),
            },
            Value::Kind(ClipKind::Fixed) => Self::Kind {
                kind: "fixed".to_string(),
            },
            Value::Byte(value) => Self::Byte { value: *value },
            Value::U16(value) => Self::U16 { value: *value },
            Value::Gain(bits) => Self::Gain { bits: *bits },
            Value::Bits(bits) => Self::Bits { bits: *bits },
            Value::Flicks(value) => Self::Flicks { value: *value },
            Value::ParamIds(ids) => Self::ParamIds { ids: ids.clone() },
            Value::OrderStep { index, id, present } => Self::OrderStep {
                index: *index,
                id: id.to_string(),
                present: *present,
            },
        }
    }
}

impl ValueRecord {
    fn to_value(&self) -> Result<Value, ActionError> {
        Ok(match self {
            Self::Meter {
                numerator,
                denominator,
            } => Value::Meter(
                Meter::try_new(*numerator, *denominator).ok_or(ActionError::CorruptJournal)?,
            ),
            Self::Tempo {
                numerator,
                denominator,
            } => Value::Tempo(
                Tempo::rational(*numerator, *denominator).ok_or(ActionError::CorruptJournal)?,
            ),
            Self::Tick { tick } => Value::Tick(Tick::new(*tick)),
            Self::Text { text } => Value::Text(text.clone()),
            Self::Id { id } => Value::Id(parse_id(id)?),
            Self::Ids { ids } => Value::Ids(
                ids.iter()
                    .map(|id| parse_id(id))
                    .collect::<Result<_, _>>()?,
            ),
            Self::Parent { id } => Value::Parent(id.as_deref().map(parse_id).transpose()?),
            Self::Audio { hash: None, .. } => Value::Audio(None),
            Self::Audio {
                hash: Some(hash),
                sample_rate,
                channels,
                frames,
            } => Value::Audio(Some(
                AudioRef::try_new(hash.clone(), *sample_rate, *channels, *frames)
                    .map_err(|_| ActionError::CorruptJournal)?,
            )),
            Self::Kind { kind } => Value::Kind(match kind.as_str() {
                "auto" => ClipKind::Auto,
                "fixed" => ClipKind::Fixed,
                _ => return Err(ActionError::CorruptJournal),
            }),
            Self::Byte { value } => Value::Byte(*value),
            Self::U16 { value } => Value::U16(*value),
            Self::Gain { bits } => Value::Gain(*bits),
            Self::Bits { bits } => Value::Bits(*bits),
            Self::Flicks { value } => Value::Flicks(*value),
            Self::ParamIds { ids } => Value::ParamIds(ids.clone()),
            Self::OrderStep { index, id, present } => Value::OrderStep {
                index: *index,
                id: parse_id(id)?,
                present: *present,
            },
        })
    }
}

fn parse_fields(fields: &[FieldRecord]) -> Result<Vec<Field>, ActionError> {
    fields
        .iter()
        .map(|field| {
            Ok(Field {
                property: parse_property(&field.property)?,
                value: field.value.to_value()?,
            })
        })
        .collect()
}

fn kind_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Track => "track",
        ObjectKind::Clip => "clip",
        ObjectKind::Note => "note",
        ObjectKind::TempoPoint => "tempo",
        ObjectKind::Content => "content",
        ObjectKind::Device => "device",
        ObjectKind::Parameter => "parameter",
    }
}

fn parse_kind(kind: &str) -> Result<ObjectKind, ActionError> {
    Ok(match kind {
        "track" => ObjectKind::Track,
        "clip" => ObjectKind::Clip,
        "note" => ObjectKind::Note,
        "tempo" => ObjectKind::TempoPoint,
        "content" => ObjectKind::Content,
        "device" => ObjectKind::Device,
        "parameter" => ObjectKind::Parameter,
        _ => return Err(ActionError::CorruptJournal),
    })
}

fn property_name(property: Property) -> &'static str {
    match property {
        Property::Meter => "meter",
        Property::TrackOrder => "track_order",
        Property::Name => "name",
        Property::Parent => "parent",
        Property::Order => "order",
        Property::ClipOrder => "clip_order",
        Property::Start => "start",
        Property::Length => "length",
        Property::Kind => "kind",
        Property::NoteOrder => "note_order",
        Property::Owner => "owner",
        Property::Pitch => "pitch",
        Property::Velocity => "velocity",
        Property::Release => "release",
        Property::Articulation => "articulation",
        Property::Tempo => "tempo",
        Property::Offset => "offset",
        Property::Gain => "gain",
        Property::FadeIn => "fade_in",
        Property::FadeOut => "fade_out",
        Property::Lane => "lane",
        Property::Content => "content",
        Property::Audio => "audio",
        Property::Takes => "takes",
        Property::Key => "key",
        Property::Minimum => "minimum",
        Property::Maximum => "maximum",
        Property::Default => "default",
        Property::Value => "value",
        Property::ParamOrder => "param_order",
        Property::DeviceKind => "device_kind",
        Property::DeviceOrder => "device_order",
        Property::Plugin => "plugin",
        Property::PluginState => "plugin_state",
        Property::Bypassed => "bypassed",
        Property::Offline => "offline",
        Property::PinnedParameters => "pinned_parameters",
    }
}

fn parse_property(property: &str) -> Result<Property, ActionError> {
    Ok(match property {
        "meter" => Property::Meter,
        "track_order" => Property::TrackOrder,
        "name" => Property::Name,
        "parent" => Property::Parent,
        "order" => Property::Order,
        "clip_order" => Property::ClipOrder,
        "start" => Property::Start,
        "length" => Property::Length,
        "kind" => Property::Kind,
        "note_order" => Property::NoteOrder,
        "owner" => Property::Owner,
        "pitch" => Property::Pitch,
        "velocity" => Property::Velocity,
        "release" => Property::Release,
        "articulation" => Property::Articulation,
        "tempo" => Property::Tempo,
        "offset" => Property::Offset,
        "gain" => Property::Gain,
        "fade_in" => Property::FadeIn,
        "fade_out" => Property::FadeOut,
        "lane" => Property::Lane,
        "content" => Property::Content,
        "audio" => Property::Audio,
        "takes" => Property::Takes,
        "key" => Property::Key,
        "minimum" => Property::Minimum,
        "maximum" => Property::Maximum,
        "default" => Property::Default,
        "value" => Property::Value,
        "param_order" => Property::ParamOrder,
        "device_kind" => Property::DeviceKind,
        "device_order" => Property::DeviceOrder,
        "plugin" => Property::Plugin,
        "plugin_state" => Property::PluginState,
        "bypassed" => Property::Bypassed,
        "offline" => Property::Offline,
        "pinned_parameters" => Property::PinnedParameters,
        _ => return Err(ActionError::CorruptJournal),
    })
}

fn key_text(id: &ObjectKey) -> String {
    match id {
        ObjectKey::Project => "project".to_string(),
        ObjectKey::Tempo(tick) => format!("tempo:{}", tick.get()),
        ObjectKey::Id(id) => id.to_string(),
    }
}

fn parse_key(text: &str) -> Result<ObjectKey, ActionError> {
    if text == "project" {
        return Ok(ObjectKey::Project);
    }
    if let Some(tick) = text.strip_prefix("tempo:") {
        let tick = tick.parse().map_err(|_| ActionError::CorruptJournal)?;
        return Ok(ObjectKey::Tempo(Tick::new(tick)));
    }
    Ok(ObjectKey::Id(parse_id(text)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use echode_model::{Id, NoteDraft, TICKS_PER_QUARTER, Tick};

    fn id(raw: u128) -> String {
        Id::from_raw(raw).to_string()
    }

    #[test]
    fn the_journal_text_is_stable_and_rejects_junk() {
        let groups = vec![
            JournalEntry {
                actions: vec![Action::AddTrack {
                    id: Id::from_raw(1),
                    name: "piano".into(),
                    device: Id::from_raw(4),
                    gain: Id::from_raw(5),
                    pan: Id::from_raw(6),
                    mute: Id::from_raw(7),
                    solo: Id::from_raw(8),
                }],
                changes: None,
            },
            JournalEntry {
                actions: vec![Action::InsertNote {
                    track: Id::from_raw(1),
                    note: Id::from_raw(2),
                    clip: Id::from_raw(3),
                    content: None,
                    draft: NoteDraft {
                        pitch: 60,
                        velocity: 100,
                        start: Tick::new(0),
                        length: Tick::new(TICKS_PER_QUARTER),
                    },
                }],
                changes: None,
            },
        ];
        let text = encode(&groups);
        assert_eq!(
            text,
            format!(
                "{{\"actions\":[{{\"type\":\"track.add\",\"id\":\"{}\",\"name\":\"piano\",\"device\":\"{}\",\"gain\":\"{}\",\"pan\":\"{}\",\"mute\":\"{}\",\"solo\":\"{}\"}}]}}\n{{\"actions\":[{{\"type\":\"note.insert\",\"track\":\"{}\",\"note\":\"{}\",\"clip\":\"{}\",\"pitch\":60,\"velocity\":100,\"start\":0,\"length\":705600}}]}}\n",
                id(1),
                id(4),
                id(5),
                id(6),
                id(7),
                id(8),
                id(1),
                id(2),
                id(3),
            )
        );
        assert_eq!(decode(&text).unwrap(), groups);
        assert_eq!(encode(&decode(&text).unwrap()), text);
        assert!(decode("{\"actions\":[]}\n").is_err());
        assert!(decode("{\"type\":\"nope\"}\n").is_err());
        assert!(decode(
            "{\"actions\":[{\"type\":\"add_track\",\"id\":\"00000000-0000-0000-0000-000000000001\",\"name\":\"piano\",\"device\":\"00000000-0000-0000-0000-000000000004\",\"gain\":\"00000000-0000-0000-0000-000000000005\",\"pan\":\"00000000-0000-0000-0000-000000000006\",\"mute\":\"00000000-0000-0000-0000-000000000007\",\"solo\":\"00000000-0000-0000-0000-000000000008\"}]}\n"
        )
        .is_err());
        assert!(decode("").unwrap().is_empty());
    }
}
