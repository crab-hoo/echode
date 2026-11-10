//! One JSON-RPC request per line.

use echode_actions::{Command, ParameterError, catalog};
use echode_history::{DocumentHost, HostError};
use echode_model::Project;
use serde_json::{Value, json};

/// JSON-RPC over a [`DocumentHost`]. The host owns the session.
pub struct ApiSession {
    host: DocumentHost,
}

impl ApiSession {
    pub fn open(directory: &std::path::Path) -> Result<Self, echode_history::PackageError> {
        Ok(Self {
            host: DocumentHost::open(directory)?,
        })
    }

    /// Answers one line. `None` means the caller sent a notification.
    pub fn reply(&self, line: &str) -> Option<String> {
        // Only JSON whitespace may surround a request; Unicode trimming can turn
        // malformed input into an executable edit.
        let line = line.trim_matches([' ', '\t', '\r', '\n']);
        if line.is_empty() {
            return None;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => return Some(error_response(None, -32700, "parse error", None)),
        };
        let Some(object) = value.as_object() else {
            return Some(error_response(None, -32600, "invalid request", None));
        };
        let has_id = object.contains_key("id");
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        // An invalid envelope is not a notification. Validate it before dispatch,
        // even when a malformed caller omitted its id.
        if !id.is_null() && !id.is_string() && !id.is_number() {
            return Some(error_response(None, -32600, "invalid request", None));
        }
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(Some(&id), -32600, "invalid request", None));
        }
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return Some(error_response(Some(&id), -32600, "invalid request", None));
        };
        if object
            .get("params")
            .is_some_and(|params| !params.is_object() && !params.is_array())
        {
            return Some(error_response(Some(&id), -32600, "invalid request", None));
        }
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let body = match method {
            "actions.list" => ok_response(&id, action_catalog()),
            "project.summary" => ok_response(&id, summary(&self.host.snapshot())),
            "protocol.version" => ok_response(&id, json!({ "version": 1 })),
            "actions.perform" => outcome(&id, perform(&self.host, &params)),
            "actions.transaction" => outcome(&id, transaction(&self.host, &params)),
            "edit.undo" => outcome(&id, undo(&self.host)),
            "edit.redo" => outcome(&id, redo(&self.host)),
            "history.usage" => outcome(&id, usage(&self.host)),
            _ => error_response(Some(&id), -32601, "method not found", None),
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
                    "schema": spec.schema(),
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

#[derive(Debug)]
enum PerformError {
    Parameters(ParameterError),
    TransactionShape,
    Host(HostError),
    Serialize(serde_json::Error),
}

impl PerformError {
    fn code(&self) -> i32 {
        match self {
            Self::Parameters(_) | Self::TransactionShape => -32602,
            Self::Host(HostError::Element { error, .. }) if error.is_parameters() => -32602,
            Self::Host(_) | Self::Serialize(_) => -32000,
        }
    }

    fn index(&self) -> Option<usize> {
        match self {
            Self::Host(HostError::Element { index, .. }) => Some(*index),
            _ => None,
        }
    }
}

impl std::fmt::Display for PerformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parameters(error) => write!(f, "{error}"),
            Self::TransactionShape => f.write_str("parameters do not match a transaction"),
            Self::Host(error) => write!(f, "{error}"),
            Self::Serialize(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PerformError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parameters(error) => Some(error),
            Self::Serialize(error) => Some(error),
            Self::Host(error) => Some(error),
            Self::TransactionShape => None,
        }
    }
}

fn outcome(id: &Value, result: Result<Value, PerformError>) -> String {
    match result {
        Ok(result) => ok_response(id, result),
        Err(error) => error_response(
            Some(id),
            error.code(),
            &error.to_string(),
            error.index().map(|index| json!({ "index": index })),
        ),
    }
}

fn perform(host: &DocumentHost, params: &Value) -> Result<Value, PerformError> {
    let command = Command::from_value(params.clone()).map_err(PerformError::Parameters)?;
    match host.perform(command) {
        Ok(action) => Ok(serde_json::to_value(action).expect("a committed action is json")),
        Err(error) => Err(PerformError::Host(error)),
    }
}

fn transaction(host: &DocumentHost, params: &Value) -> Result<Value, PerformError> {
    let Some(actions) = params.get("actions").and_then(Value::as_array) else {
        return Err(PerformError::TransactionShape);
    };
    match host.apply(actions.clone()) {
        Ok(actions) => Ok(serde_json::to_value(actions).expect("committed actions are json")),
        Err(error) => Err(PerformError::Host(error)),
    }
}

fn undo(host: &DocumentHost) -> Result<Value, PerformError> {
    host.undo().map(|()| json!({})).map_err(PerformError::Host)
}

fn redo(host: &DocumentHost) -> Result<Value, PerformError> {
    host.redo().map(|()| json!({})).map_err(PerformError::Host)
}

/// What undo and redo keep in memory (ADR 0286). Byte counts are estimates.
fn usage(host: &DocumentHost) -> Result<Value, PerformError> {
    let usage = host.history_usage().map_err(PerformError::Host)?;
    serde_json::to_value(usage).map_err(PerformError::Serialize)
}

fn ok_response(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "result": result, "id": id}).to_string()
}

fn error_response(id: Option<&Value>, code: i32, message: &str, data: Option<Value>) -> String {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({
        "jsonrpc": "2.0",
        "error": error,
        "id": id.cloned().unwrap_or(Value::Null),
    })
    .to_string()
}

#[cfg(test)]
#[path = "rpc_tests.rs"]
mod tests;
