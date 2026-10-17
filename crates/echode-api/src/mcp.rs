//! Local stdio MCP adapter. Only the existing document session performs edits.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use echode_actions::catalog;
use serde_json::{Value, json};

use crate::serve::{ApiError, MAX_REQUEST_LINE, READ_TIMEOUT, current_uid, peer_uid};

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

struct ProtocolError {
    code: i32,
    message: String,
    data: Option<Value>,
}

impl ProtocolError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
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
            writeln!(output, "{response}").map_err(io_error)?;
            output.flush().map_err(io_error)?;
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
                    .map_err(|error| ProtocolError::new(-32603, error.to_string()))?;
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
            "echode.edit.undo" | "echode.edit.redo" | "echode.project.summary" => {
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
                spec.description_en,
                schema,
                false,
            )
        })
        .collect();
    tools.extend([
        tool("echode.actions.transaction", "Edit together", "Apply an actions array atomically as one Undo. Earlier result IDs may be referenced with {ref: index, field: name}.",
             json!({"type":"object","properties":{"actions":{"type":"array","items":{"type":"object"}}},"required":["actions"],"additionalProperties":false}), false),
        tool("echode.edit.undo", "Undo", "Undo the last edit in the configured session.", empty.clone(), false),
        tool("echode.edit.redo", "Redo", "Redo the last undone edit in the configured session.", empty.clone(), false),
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
    let mut error = json!({"code":problem.code,"message":problem.message});
    if let Some(data) = problem.data {
        error["data"] = data;
    }
    json!({"jsonrpc":"2.0","id":id,"error":error})
}

fn io_error(error: std::io::Error) -> ApiError {
    ApiError::new(error.to_string())
}

fn read_line(input: &mut impl BufRead) -> Result<Option<String>, ApiError> {
    let mut line = String::new();
    let count = input
        .take(MAX_REQUEST_LINE as u64 + 1)
        .read_line(&mut line)
        .map_err(io_error)?;
    if count == 0 {
        return Ok(None);
    }
    if count > MAX_REQUEST_LINE {
        return Err(ApiError::new(
            "MCP message exceeds the single-message size limit",
        ));
    }
    if !line.ends_with('\n') {
        return Err(ApiError::new("MCP message ended without a newline"));
    }
    Ok(Some(line))
}

fn backend(socket: &Path, method: &str, params: Value) -> Result<Value, ApiError> {
    // A fresh connection per call does not occupy the single-client session
    // while an MCP client sits idle. Never retry a possibly journaled edit.
    let stream = UnixStream::connect(socket).map_err(io_error)?;
    if peer_uid(&stream)? != current_uid() {
        return Err(ApiError::new("session belongs to another user"));
    }
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(io_error)?;
    stream
        .set_write_timeout(Some(READ_TIMEOUT))
        .map_err(io_error)?;
    let mut stream = BufReader::new(stream);
    let version = exchange(&mut stream, 0, "protocol.version", json!({}))?;
    if version["result"]["version"] != 1 {
        return Err(ApiError::new("unsupported Echode session protocol"));
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
        return Err(ApiError::new(
            "session request exceeds the single-message size limit",
        ));
    }
    writeln!(stream.get_mut(), "{request}").map_err(io_error)?;
    let line =
        read_line(stream)?.ok_or_else(|| ApiError::new("session closed without a response"))?;
    let response: Value =
        serde_json::from_str(&line).map_err(|error| ApiError::new(error.to_string()))?;
    if response["jsonrpc"] != "2.0"
        || response["id"] != id
        || (response.get("result").is_some() == response.get("error").is_some())
    {
        return Err(ApiError::new("invalid session response"));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bind_socket;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn stdio_keeps_parse_errors_out_of_the_following_reply_and_closes_at_eof() {
        let input = b"{bad}\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n";
        let mut output = Vec::new();
        serve_mcp(Path::new("unused.sock"), &input[..], &mut output).unwrap();
        let lines: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["id"], 7);
        assert_eq!(lines[1]["result"], json!({}));
    }

    #[test]
    fn a_message_limit_and_truncated_line_do_not_parse_another_request() {
        // This is the transport's per-message bound, not a project/track limit.
        let oversized = vec![b'x'; MAX_REQUEST_LINE + 1];
        let mut output = Vec::new();
        assert!(serve_mcp(Path::new("unused.sock"), &oversized[..], &mut output).is_err());
        assert!(output.is_empty());
        assert!(serve_mcp(Path::new("unused.sock"), &b"{}"[..], &mut output).is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn a_wrong_session_version_never_receives_an_edit() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::path::PathBuf::from(format!(
            "/tmp/echode-mcp-version-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = bind_socket(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = BufReader::new(listener.accept().unwrap().0);
            let request: Value =
                serde_json::from_str(&read_line(&mut stream).unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "protocol.version");
            writeln!(
                stream.get_mut(),
                "{}",
                json!({"jsonrpc":"2.0","id":0,"result":{"version":2}})
            )
            .unwrap();
            assert!(
                read_line(&mut stream).unwrap().is_none(),
                "an incompatible session received an edit"
            );
        });
        let result = backend(
            &path,
            "actions.perform",
            json!({"type":"track.add","name":"must not arrive"}),
        );
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported Echode session protocol")
        );
    }

    #[test]
    fn malformed_or_mismatched_backend_results_are_not_successful_edits() {
        for response in [
            "not json\n".to_string(),
            json!({"jsonrpc":"2.0","id":9,"result":{}}).to_string() + "\n",
            json!({"jsonrpc":"2.0","id":1,"result":{},"error":{}}).to_string() + "\n",
            json!({"jsonrpc":"1.0","id":1,"result":{}}).to_string() + "\n",
            "{}".to_string(),
        ] {
            let (client, server) = UnixStream::pair().unwrap();
            let server = std::thread::spawn(move || {
                let mut server = BufReader::new(server);
                read_line(&mut server).unwrap().unwrap();
                server.get_mut().write_all(response.as_bytes()).unwrap();
            });
            assert!(
                exchange(&mut BufReader::new(client), 1, "project.summary", json!({})).is_err()
            );
            server.join().unwrap();
        }
    }
}
