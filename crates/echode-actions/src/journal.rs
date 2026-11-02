//! One JSON object per undo step.
//!
//! The same journal always produces the same bytes. Ids are hyphenated UUID
//! text and musical times are integers, so a later diff can show the edit.

use serde::{Deserialize, Serialize};

use echode_model::{Change, ChangeSet, Field, Id, ObjectKey, ObjectKind, Property, Tick, Value};

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
#[serde(deny_unknown_fields, bound(deserialize = "V: Deserialize<'de>"))]
struct GroupRecord<V = Value> {
    actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    changes: Option<Vec<ChangeRecord<V>>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineRecord<V = Value> {
    seq: u64,
    kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    actions: Vec<Action>,
    changes: Vec<ChangeRecord<V>>,
}

fn parse_id(text: &str) -> Result<Id, ActionError> {
    Id::parse(text).ok_or(ActionError::CorruptJournal)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum ChangeRecord<V = Value> {
    Create {
        kind: String,
        id: String,
        fields: Vec<FieldRecord<V>>,
    },
    Delete {
        kind: String,
        id: String,
        fields: Vec<FieldRecord<V>>,
    },
    Update {
        id: String,
        property: String,
        before: V,
        after: V,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldRecord<V = Value> {
    property: String,
    value: V,
}

fn change_records(changes: &ChangeSet) -> Vec<ChangeRecord<&Value>> {
    changes.changes().iter().map(ChangeRecord::from).collect()
}

fn parse_changes(records: &[ChangeRecord]) -> Result<ChangeSet, ActionError> {
    let mut changes = ChangeSet::new();
    for record in records {
        changes.push(record.to_change()?);
    }
    Ok(changes)
}

impl<'a> From<&'a Change> for ChangeRecord<&'a Value> {
    fn from(change: &'a Change) -> Self {
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
                property: property.as_str().to_string(),
                before,
                after,
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
                before: before.clone(),
                after: after.clone(),
            },
        })
    }
}

impl<'a> From<&'a Field> for FieldRecord<&'a Value> {
    fn from(field: &'a Field) -> Self {
        Self {
            property: field.property.as_str().to_string(),
            value: &field.value,
        }
    }
}

fn parse_fields(fields: &[FieldRecord]) -> Result<Vec<Field>, ActionError> {
    fields
        .iter()
        .map(|field| {
            Ok(Field {
                property: parse_property(&field.property)?,
                value: field.value.clone(),
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

fn parse_property(property: &str) -> Result<Property, ActionError> {
    Property::parse(property).ok_or(ActionError::CorruptJournal)
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
