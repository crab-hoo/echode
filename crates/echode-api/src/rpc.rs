//! One JSON-RPC request per line.

use echode_actions::{Action, ActionError, Command, Session, catalog, encode};
use echode_model::{Id, Meter, NoteDraft, Project, Tempo, Tick};
use serde::Deserialize;
use serde_json::{Value, json};

/// A project that actions can change.
pub struct ApiSession {
    inner: Session,
}

impl ApiSession {
    pub fn open(project: Project) -> Self {
        Self {
            inner: Session::open(project),
        }
    }

    /// Answers one line. `None` means the caller sent a notification.
    pub fn reply(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => return Some(error_response(None, -32700, "parse error")),
        };
        let Some(object) = value.as_object() else {
            return Some(error_response(None, -32600, "invalid request"));
        };
        let has_id = object.contains_key("id");
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return respond(has_id, error_response(Some(&id), -32600, "invalid request"));
        }
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return respond(has_id, error_response(Some(&id), -32600, "invalid request"));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let body = match method {
            "actions.list" => ok_response(&id, action_catalog()),
            "project.summary" => ok_response(&id, summary(self.inner.project())),
            "actions.perform" => match perform(&mut self.inner, &params) {
                Ok(result) => ok_response(&id, result),
                Err(PerformError::Params(message)) => error_response(Some(&id), -32602, &message),
                Err(PerformError::Rejected(message)) => error_response(Some(&id), -32000, &message),
            },
            _ => error_response(Some(&id), -32601, "method not found"),
        };
        respond(has_id, body)
    }
}

fn respond(has_id: bool, body: String) -> Option<String> {
    if has_id { Some(body) } else { None }
}

fn action_catalog() -> Value {
    Value::Array(
        catalog()
            .iter()
            .map(|spec| {
                json!({
                    "id": spec.id,
                    "name_ja": spec.name_ja,
                    "name_en": spec.name_en,
                    "description_ja": spec.description_ja,
                    "description_en": spec.description_en,
                })
            })
            .collect(),
    )
}

fn summary(project: &Project) -> Value {
    json!({
        "meter": {
            "numerator": project.meter().numerator(),
            "denominator": project.meter().denominator(),
        },
        "tracks": project.tracks().iter().map(|track| json!({
            "id": track.id.to_string(),
            "name": track.name,
            "parent": track.parent.map(|id| id.to_string()),
            "clips": track.clips.len(),
        })).collect::<Vec<_>>(),
    })
}

enum PerformError {
    Params(String),
    Rejected(String),
}

fn perform(session: &mut Session, params: &Value) -> Result<Value, PerformError> {
    let requested: Requested = serde_json::from_value(params.clone())
        .map_err(|_| PerformError::Params("parameters do not match an action".into()))?;
    let command = requested.into_command()?;
    match session.perform(command) {
        Ok(action) => Ok(committed(&action)),
        Err(ActionError::Model(error)) => Err(PerformError::Rejected(error.to_string())),
        Err(error) => Err(PerformError::Rejected(error.to_string())),
    }
}

fn committed(action: &Action) -> Value {
    let text = encode(&[vec![action.clone()]]);
    let value: Value = serde_json::from_str(text.trim()).expect("a committed action is valid json");
    value["actions"][0].clone()
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Requested {
    AddTrack {
        name: String,
    },
    AddChild {
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

impl Requested {
    fn into_command(self) -> Result<Command, PerformError> {
        Ok(match self {
            Self::AddTrack { name } => Command::AddTrack { name },
            Self::AddChild { parent, name } => Command::AddChild {
                parent: parse_id(&parent)?,
                name,
            },
            Self::SetParent { track, parent } => Command::SetParent {
                track: parse_id(&track)?,
                parent: parent.as_deref().map(parse_id).transpose()?,
            },
            Self::SetMeter {
                numerator,
                denominator,
            } => Command::SetMeter {
                meter: Meter::try_new(numerator, denominator).ok_or_else(|| {
                    PerformError::Params("meter does not divide into ticks".into())
                })?,
            },
            Self::SetTempo {
                at,
                numerator,
                denominator,
            } => Command::SetTempo {
                at: Tick::new(at),
                tempo: Tempo::rational(numerator, denominator).ok_or_else(|| {
                    PerformError::Params("tempo is outside the allowed range".into())
                })?,
            },
            Self::InsertNote {
                track,
                pitch,
                velocity,
                start,
                length,
            } => Command::InsertNote {
                track: parse_id(&track)?,
                draft: NoteDraft {
                    pitch,
                    velocity,
                    start: Tick::new(start),
                    length: Tick::new(length),
                },
            },
            Self::DeleteNote { track, note } => Command::DeleteNote {
                track: parse_id(&track)?,
                note: parse_id(&note)?,
            },
            Self::ResizeClip {
                track,
                clip,
                start,
                length,
            } => Command::ResizeClip {
                track: parse_id(&track)?,
                clip: parse_id(&clip)?,
                start: Tick::new(start),
                length: Tick::new(length),
            },
        })
    }
}

fn parse_id(text: &str) -> Result<Id, PerformError> {
    Id::parse(text).ok_or_else(|| PerformError::Params(format!("invalid id {text}")))
}

fn ok_response(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "result": result, "id": id}).to_string()
}

fn error_response(id: Option<&Value>, code: i32, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "error": {"code": code, "message": message},
        "id": id.cloned().unwrap_or(Value::Null),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(session: &mut ApiSession, line: &str) -> Value {
        serde_json::from_str(&session.reply(line).expect("response")).unwrap()
    }

    #[test]
    fn the_catalog_lists_track_add() {
        let mut session = ApiSession::open(Project::new());
        let body = response(
            &mut session,
            r#"{"jsonrpc":"2.0","method":"actions.list","id":1}"#,
        );
        let ids: Vec<&str> = body["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"track.add"));
        assert_eq!(body["id"], 1);
    }

    #[test]
    fn performing_an_action_shows_up_in_the_summary() {
        let mut session = ApiSession::open(Project::new());
        let added = response(
            &mut session,
            r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"add_track","name":"piano"},"id":"a"}"#,
        );
        let id = added["result"]["id"].as_str().unwrap();
        let body = response(
            &mut session,
            r#"{"jsonrpc":"2.0","method":"project.summary","id":2}"#,
        );
        assert_eq!(body["result"]["tracks"][0]["id"], id);
        assert_eq!(body["result"]["tracks"][0]["name"], "piano");
        assert!(body["result"]["tracks"][0]["parent"].is_null());
    }

    #[test]
    fn a_rejected_edit_leaves_the_project_unchanged() {
        let mut session = ApiSession::open(Project::new());
        response(
            &mut session,
            r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"add_track","name":"piano"},"id":1}"#,
        );
        let added = response(
            &mut session,
            r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"add_track","name":"piano"},"id":2}"#,
        );
        let track = added["result"]["id"].as_str().unwrap();
        let note = format!(
            r#"{{"jsonrpc":"2.0","method":"actions.perform","params":{{"type":"insert_note","track":"{track}","pitch":60,"velocity":100,"start":0,"length":705600}},"id":3}}"#
        );
        response(&mut session, &note);
        let rejected = response(
            &mut session,
            r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"set_meter","numerator":3,"denominator":4},"id":4}"#,
        );
        assert_eq!(rejected["error"]["code"], -32000);
        let body = response(
            &mut session,
            r#"{"jsonrpc":"2.0","method":"project.summary","id":5}"#,
        );
        assert_eq!(body["result"]["meter"]["numerator"], 4);
        assert_eq!(body["result"]["tracks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn bad_json_and_unknown_methods_are_errors() {
        let mut session = ApiSession::open(Project::new());
        let parsed = response(&mut session, "not json");
        assert_eq!(parsed["error"]["code"], -32700);
        assert!(parsed["id"].is_null());
        let missing = response(&mut session, r#"{"jsonrpc":"2.0","method":"nope","id":7}"#);
        assert_eq!(missing["error"]["code"], -32601);
        assert!(
            session
                .reply(r#"{"jsonrpc":"2.0","method":"project.summary"}"#)
                .is_none()
        );
    }
}
