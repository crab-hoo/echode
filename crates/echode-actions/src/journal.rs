//! One JSON object per undo step.
//!
//! The same journal always produces the same bytes. Ids are hyphenated UUID
//! text and musical times are integers, so a later diff can show the edit.

use serde::{Deserialize, Serialize};

use echode_model::{Id, Meter, NoteDraft, Tempo, Tick};

use crate::action::{Action, ActionError};

/// Writes the journal. An empty journal is an empty string.
pub fn encode(journal: &[Vec<Action>]) -> String {
    let mut out = String::new();
    for group in journal {
        debug_assert!(!group.is_empty());
        let record = GroupRecord {
            actions: group.iter().map(ActionRecord::from).collect(),
        };
        let line = serde_json::to_string(&record).expect("action records have no failing fields");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Reads a journal written by [`encode`].
pub fn decode(text: &str) -> Result<Vec<Vec<Action>>, ActionError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.lines().map(decode_group).collect()
}

fn decode_group(line: &str) -> Result<Vec<Action>, ActionError> {
    let record: GroupRecord =
        serde_json::from_str(line).map_err(|_| ActionError::CorruptJournal)?;
    if record.actions.is_empty() {
        return Err(ActionError::CorruptJournal);
    }
    record.actions.into_iter().map(Action::try_from).collect()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupRecord {
    actions: Vec<ActionRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ActionRecord {
    AddTrack {
        id: String,
        name: String,
    },
    AddChild {
        id: String,
        parent: String,
        name: String,
    },
    SetParent {
        track: String,
        parent: Option<String>,
    },
    SetMeter {
        numerator: u32,
        denominator: u32,
    },
    SetTempo {
        at: i64,
        numerator: u32,
        denominator: u32,
    },
    InsertNote {
        track: String,
        note: String,
        clip: String,
        pitch: u8,
        velocity: u8,
        start: i64,
        length: i64,
    },
    DeleteNote {
        track: String,
        note: String,
    },
    ResizeClip {
        track: String,
        clip: String,
        start: i64,
        length: i64,
    },
}

impl From<&Action> for ActionRecord {
    fn from(action: &Action) -> Self {
        match action {
            Action::AddTrack { id, name } => Self::AddTrack {
                id: id.to_string(),
                name: name.clone(),
            },
            Action::AddChild { id, parent, name } => Self::AddChild {
                id: id.to_string(),
                parent: parent.to_string(),
                name: name.clone(),
            },
            Action::SetParent { track, parent } => Self::SetParent {
                track: track.to_string(),
                parent: parent.map(|id| id.to_string()),
            },
            Action::SetMeter { meter } => Self::SetMeter {
                numerator: meter.numerator(),
                denominator: meter.denominator(),
            },
            Action::SetTempo { at, tempo } => Self::SetTempo {
                at: at.get(),
                numerator: tempo.numerator(),
                denominator: tempo.denominator(),
            },
            Action::InsertNote {
                track,
                note,
                clip,
                draft,
            } => Self::InsertNote {
                track: track.to_string(),
                note: note.to_string(),
                clip: clip.to_string(),
                pitch: draft.pitch,
                velocity: draft.velocity,
                start: draft.start.get(),
                length: draft.length.get(),
            },
            Action::DeleteNote { track, note } => Self::DeleteNote {
                track: track.to_string(),
                note: note.to_string(),
            },
            Action::ResizeClip {
                track,
                clip,
                start,
                length,
            } => Self::ResizeClip {
                track: track.to_string(),
                clip: clip.to_string(),
                start: start.get(),
                length: length.get(),
            },
        }
    }
}

impl TryFrom<ActionRecord> for Action {
    type Error = ActionError;

    fn try_from(record: ActionRecord) -> Result<Self, Self::Error> {
        Ok(match record {
            ActionRecord::AddTrack { id, name } => Self::AddTrack {
                id: parse_id(&id)?,
                name,
            },
            ActionRecord::AddChild { id, parent, name } => Self::AddChild {
                id: parse_id(&id)?,
                parent: parse_id(&parent)?,
                name,
            },
            ActionRecord::SetParent { track, parent } => Self::SetParent {
                track: parse_id(&track)?,
                parent: parent.as_deref().map(parse_id).transpose()?,
            },
            ActionRecord::SetMeter {
                numerator,
                denominator,
            } => Self::SetMeter {
                meter: Meter::try_new(numerator, denominator).ok_or(ActionError::CorruptJournal)?,
            },
            ActionRecord::SetTempo {
                at,
                numerator,
                denominator,
            } => Self::SetTempo {
                at: Tick::new(at),
                tempo: Tempo::rational(numerator, denominator)
                    .ok_or(ActionError::CorruptJournal)?,
            },
            ActionRecord::InsertNote {
                track,
                note,
                clip,
                pitch,
                velocity,
                start,
                length,
            } => Self::InsertNote {
                track: parse_id(&track)?,
                note: parse_id(&note)?,
                clip: parse_id(&clip)?,
                draft: NoteDraft {
                    pitch,
                    velocity,
                    start: Tick::new(start),
                    length: Tick::new(length),
                },
            },
            ActionRecord::DeleteNote { track, note } => Self::DeleteNote {
                track: parse_id(&track)?,
                note: parse_id(&note)?,
            },
            ActionRecord::ResizeClip {
                track,
                clip,
                start,
                length,
            } => Self::ResizeClip {
                track: parse_id(&track)?,
                clip: parse_id(&clip)?,
                start: Tick::new(start),
                length: Tick::new(length),
            },
        })
    }
}

fn parse_id(text: &str) -> Result<Id, ActionError> {
    Id::parse(text).ok_or(ActionError::CorruptJournal)
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
            vec![Action::AddTrack {
                id: Id::from_raw(1),
                name: "piano".into(),
            }],
            vec![Action::InsertNote {
                track: Id::from_raw(1),
                note: Id::from_raw(2),
                clip: Id::from_raw(3),
                draft: NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::new(TICKS_PER_QUARTER),
                },
            }],
        ];
        let text = encode(&groups);
        assert_eq!(
            text,
            format!(
                "{{\"actions\":[{{\"type\":\"add_track\",\"id\":\"{}\",\"name\":\"piano\"}}]}}\n{{\"actions\":[{{\"type\":\"insert_note\",\"track\":\"{}\",\"note\":\"{}\",\"clip\":\"{}\",\"pitch\":60,\"velocity\":100,\"start\":0,\"length\":705600}}]}}\n",
                id(1),
                id(1),
                id(2),
                id(3),
            )
        );
        assert_eq!(decode(&text).unwrap(), groups);
        assert_eq!(encode(&decode(&text).unwrap()), text);
        assert!(decode("{\"actions\":[]}\n").is_err());
        assert!(decode("{\"type\":\"nope\"}\n").is_err());
        assert!(decode("").unwrap().is_empty());
    }
}
