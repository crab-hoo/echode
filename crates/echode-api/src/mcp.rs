//! Local stdio MCP adapter. Only the existing document session performs edits.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use echode_actions::catalog;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiIoOperation};
use crate::serve::{MAX_REQUEST_LINE, READ_TIMEOUT, current_uid, peer_uid};

const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const SUMMARY_URI: &str = "echode://project/summary";
const TRACKS_URI: &str = "echode://project/tracks";

#[derive(Default, PartialEq)]
enum Lifecycle {
    #[default]
    Fresh,
    Initializing,
    Ready,
}

#[derive(Debug)]
enum ProtocolMessage {
    Local(&'static str),
    Session(Box<ApiError>),
}

impl std::fmt::Display for ProtocolMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(message) => f.write_str(message),
            Self::Session(error) => write!(f, "{error}"),
        }
    }
}

#[derive(Debug)]
struct ProtocolError {
    code: i32,
    message: ProtocolMessage,
    data: Option<Value>,
}

impl ProtocolError {
    fn new(code: i32, message: &'static str) -> Self {
        Self {
            code,
            message: ProtocolMessage::Local(message),
            data: None,
        }
    }

    fn session(error: ApiError) -> Self {
        Self {
            code: -32603,
            message: ProtocolMessage::Session(Box::new(error)),
            data: None,
        }
    }
}

struct McpSession<'a> {
    socket: &'a Path,
    lifecycle: Lifecycle,
    legacy_ids: BTreeSet<String>,
}

/// One newline-delimited MCP stream. EOF closes only this adapter, not the
/// configured document server. No diagnostics are written to `output`.
pub fn serve_mcp(
    socket: &Path,
    mut input: impl BufRead,
    mut output: impl Write,
) -> Result<(), ApiError> {
    let mut session = McpSession {
        socket,
        lifecycle: Lifecycle::Fresh,
        legacy_ids: BTreeSet::new(),
    };
    while let Some(line) = read_line(&mut input)? {
        if let Some(response) = session.reply(&line) {
            writeln!(output, "{response}")
                .map_err(|error| ApiError::io(ApiIoOperation::WriteMcpReply, error))?;
            output
                .flush()
                .map_err(|error| ApiError::io(ApiIoOperation::FlushMcpReply, error))?;
        }
    }
    Ok(())
}

impl McpSession<'_> {
    fn reply(&mut self, line: &str) -> Option<Value> {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                return Some(error(
                    &Value::Null,
                    ProtocolError::new(-32700, "parse error"),
                ));
            }
        };
        let valid_id = value
            .get("id")
            .filter(|id| id.is_string() || id.is_i64() || id.is_u64());
        let id = valid_id.cloned().unwrap_or(Value::Null);
        if !value.is_object() || value["jsonrpc"] != "2.0" || !value["method"].is_string() {
            return Some(error(&id, ProtocolError::new(-32600, "invalid request")));
        }
        let method = value["method"].as_str()?;
        if value.get("id").is_none() {
            // Requests disguised as notifications must never execute an edit.
            if method == "notifications/initialized"
                && self.lifecycle == Lifecycle::Initializing
                && value.get("params").is_none_or(Value::is_object)
            {
                self.lifecycle = Lifecycle::Ready;
            }
            return None;
        }
        if valid_id.is_none() {
            return Some(error(
                &Value::Null,
                ProtocolError::new(-32600, "request id must be a string or integer"),
            ));
        }
        let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Some(error(
                &id,
                ProtocolError::new(-32602, "params must be an object"),
            ));
        }
        let modern = params
            .get("_meta")
            .and_then(|meta| meta.get(VERSION_KEY))
            .is_some();
        let result = if modern {
            validate_modern(&params).and_then(|()| self.dispatch(method, &params, true))
        } else {
            self.legacy(&id, method, &params)
        };
        Some(match result {
            Ok(mut result) => {
                if modern {
                    result["resultType"] = json!("complete");
                    result["_meta"] = json!({"io.modelcontextprotocol/serverInfo": server_info()});
                    if matches!(
                        method,
                        "server/discover" | "tools/list" | "resources/list" | "resources/read"
                    ) {
                        result["ttlMs"] = json!(0);
                        result["cacheScope"] = json!("private");
                    }
                }
                json!({"jsonrpc":"2.0", "id":id, "result":result})
            }
            Err(problem) => error(&id, problem),
        })
    }

    fn legacy(&mut self, id: &Value, method: &str, params: &Value) -> Result<Value, ProtocolError> {
        if self.legacy_ids.contains(&id.to_string()) {
            return Err(ProtocolError::new(-32600, "request id was already used"));
        }
        if method == "initialize" {
            if self.lifecycle != Lifecycle::Fresh {
                return Err(ProtocolError::new(-32600, "already initialized"));
            }
            if !params["protocolVersion"].is_string()
                || !params["capabilities"].is_object()
                || !params["clientInfo"]["name"].is_string()
                || !params["clientInfo"]["version"].is_string()
            {
                return Err(ProtocolError::new(
                    -32602,
                    "initialize requires protocolVersion, capabilities and clientInfo",
                ));
            }
            self.lifecycle = Lifecycle::Initializing;
            self.legacy_ids.insert(id.to_string());
            return Ok(
                json!({"protocolVersion":LEGACY,"capabilities":capabilities(),"serverInfo":server_info()}),
            );
        }
        if method != "ping" && self.lifecycle != Lifecycle::Ready {
            return Err(ProtocolError::new(
                -32602,
                "initialize first, or supply modern per-request protocol metadata",
            ));
        }
        if !self.legacy_ids.insert(id.to_string()) {
            return Err(ProtocolError::new(-32600, "request id was already used"));
        }
        self.dispatch(method, params, false)
    }

    fn dispatch(&self, method: &str, params: &Value, modern: bool) -> Result<Value, ProtocolError> {
        match method {
            "server/discover" if modern => {
                Ok(json!({"supportedVersions":[MODERN, LEGACY], "capabilities":capabilities()}))
            }
            "ping" if !modern => Ok(json!({})),
            "tools/list" => {
                no_cursor(params)?;
                Ok(json!({"tools":tools()}))
            }
            "tools/call" => self.call_tool(params),
            "resources/list" => {
                no_cursor(params)?;
                Ok(json!({"resources":[
                    {"uri":SUMMARY_URI,"name":"Project summary","mimeType":"application/json"},
                    {"uri":TRACKS_URI,"name":"Tracks","mimeType":"application/json"}
                ]}))
            }
            "resources/read" => {
                let uri = params["uri"]
                    .as_str()
                    .ok_or_else(|| ProtocolError::new(-32602, "uri must be a string"))?;
                if !matches!(uri, SUMMARY_URI | TRACKS_URI) {
                    return Err(ProtocolError::new(
                        if modern { -32602 } else { -32002 },
                        "resource not found",
                    ));
                }
                let response = backend(self.socket, "project.summary", json!({}))
                    .map_err(ProtocolError::session)?;
                let summary = response
                    .get("result")
                    .ok_or_else(|| ProtocolError::new(-32603, "session refused project summary"))?;
                let data = if uri == TRACKS_URI {
                    &summary["tracks"]
                } else {
                    summary
                };
                Ok(
                    json!({"contents":[{"uri":uri,"mimeType":"application/json","text":data.to_string()}]}),
                )
            }
            _ => Err(ProtocolError::new(-32601, "method not found")),
        }
    }

    fn call_tool(&self, params: &Value) -> Result<Value, ProtocolError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| ProtocolError::new(-32602, "tool name must be a string"))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Err(ProtocolError::new(
                -32602,
                "tool arguments must be an object",
            ));
        }
        let (method, arguments) = match name {
            "echode.actions.transaction" => {
                if arguments
                    .as_object()
                    .is_some_and(|object| object.keys().any(|key| key != "actions"))
                {
                    return Ok(tool_result(
                        json!({"error":{"message":"transaction accepts only an actions array"}}),
                        true,
                    ));
                }
                ("actions.transaction", arguments)
            }
            "echode.edit.undo"
            | "echode.edit.redo"
            | "echode.history.usage"
            | "echode.project.summary" => {
                if !arguments
                    .as_object()
                    .is_some_and(|object| object.is_empty())
                {
                    return Ok(tool_result(
                        json!({"error":{"message":"this tool takes no arguments"}}),
                        true,
                    ));
                }
                (name.strip_prefix("echode.").unwrap_or(name), arguments)
            }
            _ => {
                let action = name
                    .strip_prefix("echode.")
                    .filter(|id| catalog().iter().any(|spec| spec.id == *id))
                    .ok_or_else(|| ProtocolError::new(-32602, "unknown tool"))?;
                let mut arguments = arguments;
                if arguments.get("type").is_some() {
                    return Ok(tool_result(
                        json!({"error":{"message":"the action type is selected by the tool name"}}),
                        true,
                    ));
                }
                arguments["type"] = json!(action);
                ("actions.perform", arguments)
            }
        };
        match backend(self.socket, method, arguments) {
            Ok(response) => {
                let failed = response.get("error").is_some();
                let data = if failed {
                    json!({"error":response["error"]})
                } else {
                    json!({"result":response["result"]})
                };
                Ok(tool_result(data, failed))
            }
            Err(error) => Ok(tool_result(
                json!({"error":{
                    "message":format!("Session request failed: {error}. Inspect the project before retrying an edit; its outcome may be unknown.")
                }}),
                true,
            )),
        }
    }
}

fn validate_modern(params: &Value) -> Result<(), ProtocolError> {
    let meta = &params["_meta"];
    let version = meta[VERSION_KEY]
        .as_str()
        .ok_or_else(|| ProtocolError::new(-32602, "protocol version must be a string"))?;
    if version != MODERN {
        let mut error = ProtocolError::new(-32022, "unsupported protocol version");
        error.data = Some(json!({"supported":[MODERN, LEGACY], "requested":version}));
        return Err(error);
    }
    if !meta[CAPABILITIES_KEY].is_object() {
        return Err(ProtocolError::new(
            -32602,
            "clientCapabilities must be an object on every modern request",
        ));
    }
    Ok(())
}

fn no_cursor(params: &Value) -> Result<(), ProtocolError> {
    if params.get("cursor").is_some() {
        return Err(ProtocolError::new(
            -32602,
            "this list has no continuation cursor",
        ));
    }
    Ok(())
}

fn server_info() -> Value {
    json!({"name":"echode", "version":env!("CARGO_PKG_VERSION")})
}
fn capabilities() -> Value {
    json!({"tools":{},"resources":{}})
}

fn tools() -> Vec<Value> {
    let empty = json!({"type":"object","additionalProperties":false});
    let mut tools: Vec<_> = catalog()
        .iter()
        .map(|spec| {
            let mut schema = spec.schema();
            schema["properties"]
                .as_object_mut()
                .expect("action properties")
                .remove("type");
            if let Some(required) = schema["required"].as_array_mut() {
                required.retain(|key| key != "type");
            }
            tool(
                &format!("echode.{}", spec.id),
                spec.name_en,
                &format!(
                    "{}. An already satisfied edit succeeds without adding Undo history.",
                    spec.description_en
                ),
                schema,
                false,
            )
        })
        .collect();
    tools.extend([
        tool("echode.actions.transaction", "Edit together", "Apply an actions array atomically as one Undo when the project changes. A group with no net change succeeds without adding Undo history. Earlier result IDs may be referenced with {ref: index, field: name}.",
             json!({"type":"object","properties":{"actions":{"type":"array","items":{"type":"object"}}},"required":["actions"],"additionalProperties":false}), false),
        tool("echode.edit.undo", "Undo", "Undo the last edit in the configured session.", empty.clone(), false),
        tool("echode.edit.redo", "Redo", "Redo the last undone edit in the configured session.", empty.clone(), false),
        tool("echode.history.usage", "History memory", "Read how many Undo and Redo steps the session keeps and an estimate of the memory only they hold.", empty.clone(), true),
        tool("echode.project.summary", "Project summary", "Read the configured project's meter and tracks without opening an audio device.", empty, true),
    ]);
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    tools
}

fn tool(name: &str, title: &str, description: &str, schema: Value, read_only: bool) -> Value {
    json!({"name":name,"title":title,"description":description,"inputSchema":schema,
           "annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"idempotentHint":read_only,"openWorldHint":false}})
}

fn tool_result(data: Value, failed: bool) -> Value {
    json!({"content":[{"type":"text","text":data.to_string()}],"structuredContent":data,"isError":failed})
}

fn error(id: &Value, problem: ProtocolError) -> Value {
    let mut error = json!({"code":problem.code,"message":problem.message.to_string()});
    if let Some(data) = problem.data {
        error["data"] = data;
    }
    json!({"jsonrpc":"2.0","id":id,"error":error})
}

fn read_line(input: &mut impl BufRead) -> Result<Option<String>, ApiError> {
    let mut line = String::new();
    let count = input
        .take(MAX_REQUEST_LINE as u64 + 1)
        .read_line(&mut line)
        .map_err(|error| ApiError::io(ApiIoOperation::ReadMcpMessage, error))?;
    if count == 0 {
        return Ok(None);
    }
    if count > MAX_REQUEST_LINE {
        return Err(ApiError::McpMessageTooLong);
    }
    if !line.ends_with('\n') {
        return Err(ApiError::McpMessageIncomplete);
    }
    Ok(Some(line))
}

fn backend(socket: &Path, method: &str, params: Value) -> Result<Value, ApiError> {
    // A fresh connection per call does not occupy the single-client session
    // while an MCP client sits idle. Never retry a possibly journaled edit.
    let stream = UnixStream::connect(socket)
        .map_err(|error| ApiError::io(ApiIoOperation::ConnectSession, error))?;
    if peer_uid(&stream)? != current_uid() {
        return Err(ApiError::SessionFromAnotherUser);
    }
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|error| ApiError::io(ApiIoOperation::SetReadTimeout, error))?;
    stream
        .set_write_timeout(Some(READ_TIMEOUT))
        .map_err(|error| ApiError::io(ApiIoOperation::SetWriteTimeout, error))?;
    let mut stream = BufReader::new(stream);
    let version = exchange(&mut stream, 0, "protocol.version", json!({}))?;
    if version["result"]["version"] != 1 {
        return Err(ApiError::UnsupportedSessionProtocol);
    }
    exchange(&mut stream, 1, method, params)
}

fn exchange(
    stream: &mut BufReader<UnixStream>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, ApiError> {
    let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    if request.to_string().len() + 1 > MAX_REQUEST_LINE {
        return Err(ApiError::SessionRequestTooLong);
    }
    writeln!(stream.get_mut(), "{request}")
        .map_err(|error| ApiError::io(ApiIoOperation::WriteSessionRequest, error))?;
    let line = read_line(stream)?.ok_or(ApiError::SessionClosed)?;
    let response: Value = serde_json::from_str(&line).map_err(ApiError::SessionJson)?;
    if response["jsonrpc"] != "2.0"
        || response["id"] != id
        || (response.get("result").is_some() == response.get("error").is_some())
        || response.get("error").is_some_and(|error| {
            !error.is_object()
                || !(error["code"].is_i64() || error["code"].is_u64())
                || !error["message"].is_string()
        })
    {
        return Err(ApiError::InvalidSessionResponse);
    }
    Ok(response)
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
