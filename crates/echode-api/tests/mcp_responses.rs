//! Invalid backend responses are not authoritative outcomes of an MCP edit.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use echode_api::{ApiSession, bind_socket, serve_mcp};
use echode_history::write_package;
use echode_model::{Id, Project};
use serde_json::{Value, json};

#[path = "support/mcp_envelopes.rs"]
mod envelopes;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("echode-mcp-responses-{}", Id::generate()));
        write_package(&path, &Project::new()).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Socket(PathBuf);

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn call(session: &ApiSession, method: &str) -> Value {
    let request = json!({"jsonrpc":"2.0", "id":1, "method":method});
    let response: Value =
        serde_json::from_str(&session.reply(&request.to_string()).unwrap()).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn track_count(session: &ApiSession) -> usize {
    call(session, "project.summary")["tracks"]
        .as_array()
        .unwrap()
        .len()
}

fn backend_response(response: Value, legacy: bool, session: Option<Arc<ApiSession>>) -> Value {
    let socket = Socket(PathBuf::from(format!(
        "/tmp/echode-mcp-responses-{}.sock",
        Id::generate()
    )));
    let listener = bind_socket(&socket.0).unwrap();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        // These deadlines bound only this socket regression, not product work.
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).unwrap();
        let version: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(version["method"], "protocol.version");
        writeln!(
            stream.get_mut(),
            "{}",
            json!({"jsonrpc":"2.0", "id":0, "result":{"version":1}})
        )
        .unwrap();
        line.clear();
        stream.read_line(&mut line).unwrap();
        let edit: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(edit["method"], "actions.perform");
        assert_eq!(edit["params"]["name"], "recorded once");
        if let Some(session) = session {
            // Commit through the real host, then replace only the response envelope.
            let committed: Value = serde_json::from_str(&session.reply(&line).unwrap()).unwrap();
            assert!(committed.get("result").is_some(), "{committed}");
        }
        writeln!(stream.get_mut(), "{response}").unwrap();
        // This complete line needs no half-close; the peer may already have closed.
        let mut following = String::new();
        stream.read_to_string(&mut following).unwrap();
        assert!(
            following.is_empty(),
            "an uncertain edit must not be replayed"
        );
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "an uncertain edit must not reconnect for a retry"
        );
    });
    let mut request = json!({"jsonrpc":"2.0", "id":"edit", "method":"tools/call", "params":{
        "name":"echode.track.add", "arguments":{"name":"recorded once"}
    }});
    let prefix = if legacy {
        concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":\"init\",\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"regression\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n"
        )
    } else {
        request["params"]["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientCapabilities":{}
        });
        ""
    };
    let input = format!(
        "{prefix}{request}\n{{\"jsonrpc\":\"2.0\",\"id\":\"after\",\"method\":\"ping\"}}\n"
    );
    let mut output = Vec::new();
    let result = serve_mcp(&socket.0, input.as_bytes(), &mut output);
    worker.join().unwrap();
    result.unwrap();
    let replies: Vec<Value> = std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 2 + usize::from(legacy));
    if legacy {
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
    }
    let after = replies.last().unwrap();
    assert_eq!(after["id"], "after");
    assert_eq!(after["result"], json!({}));
    let reply = &replies[usize::from(legacy)];
    assert_eq!(reply["id"], "edit");
    assert_eq!(
        reply.get("result").unwrap().get("resultType").is_none(),
        legacy
    );
    reply["result"].clone()
}

#[test]
fn contract_malformed_backend_errors_preserve_uncertain_edits_without_retrying() {
    let fixture = Fixture::new();
    let session = Arc::new(ApiSession::open(&fixture.0).unwrap());
    let mut count = 0;
    for legacy in [false, true] {
        for error in [
            Value::Null,
            json!(false),
            json!("refused"),
            json!([]),
            json!({}),
            json!({"message":"missing code"}),
            json!({"code":-32000}),
            json!({"code":"-32000", "message":"wrong code type"}),
            json!({"code":true, "message":"wrong code type"}),
            json!({"code":-32000.5, "message":"fractional code"}),
            json!({"code":-32000, "message":null}),
            json!({"code":-32000, "message":{}}),
        ] {
            let result = backend_response(
                json!({"jsonrpc":"2.0", "id":1, "error":error}),
                legacy,
                Some(Arc::clone(&session)),
            );
            assert_eq!(result["isError"], true);
            let message = result["structuredContent"]["error"]["message"]
                .as_str()
                .unwrap_or("");
            assert!(
                message.contains("invalid session response")
                    && message.contains("outcome may be unknown"),
                "malformed backend errors must report an uncertain edit: {error} -> {result}"
            );
            assert!(message.contains("Inspect the project before retrying an edit"));
            count += 1;
            assert_eq!(track_count(&session), count);
            call(&session, "edit.undo");
            assert_eq!(track_count(&session), count - 1);
            call(&session, "edit.redo");
            assert_eq!(track_count(&session), count);
        }
    }
    let expected = call(&session, "project.summary");
    drop(session);
    let reopened = ApiSession::open(&fixture.0).unwrap();
    assert_eq!(call(&reopened, "project.summary"), expected);
}

#[test]
fn valid_backend_errors_keep_their_code_message_and_optional_data() {
    for legacy in [false, true] {
        for error in [
            json!({"code":-32000, "message":"edit refused"}),
            json!({"code":-32602, "message":"invalid arguments", "data":{"index":2}}),
            json!({"code":i64::MIN, "message":"", "data":null}),
            json!({"code":u64::MAX, "message":"extension code", "data":[1, 2]}),
        ] {
            let result = backend_response(
                json!({"jsonrpc":"2.0", "id":1, "error":error, "extension":{"future":true}}),
                legacy,
                None,
            );
            assert_eq!(result["isError"], true);
            assert_eq!(result["structuredContent"], json!({"error":error}));
            let text: Value =
                serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(text, result["structuredContent"]);
        }
    }
}

#[test]
fn valid_backend_results_are_not_constrained_to_error_object_shapes() {
    for legacy in [false, true] {
        for value in [
            Value::Null,
            json!(true),
            json!([1, 2]),
            json!({"code":"data"}),
        ] {
            let result = backend_response(
                json!({"jsonrpc":"2.0", "id":1, "result":value}),
                legacy,
                None,
            );
            assert_eq!(result["isError"], false);
            assert_eq!(result["structuredContent"], json!({"result":value}));
        }
    }
}

#[test]
fn contract_invalid_backend_envelopes_preserve_uncertain_edits_without_retrying() {
    let fixture = Fixture::new();
    let session = Arc::new(ApiSession::open(&fixture.0).unwrap());
    let mut count = 0;
    for legacy in [false, true] {
        for response in envelopes::invalid_envelopes(1, json!({"accepted":true})) {
            let result = backend_response(response.clone(), legacy, Some(Arc::clone(&session)));
            let message = result["structuredContent"]["error"]["message"]
                .as_str()
                .unwrap_or("");
            assert!(
                result["isError"] == true
                    && message.contains("invalid session response")
                    && message.contains("outcome may be unknown"),
                "invalid backend envelopes must report an uncertain edit: {response} -> {result}"
            );
            assert!(message.contains("Inspect the project before retrying an edit"));
            count += 1;
            assert_eq!(track_count(&session), count);
            call(&session, "edit.undo");
            assert_eq!(track_count(&session), count - 1);
            call(&session, "edit.redo");
            assert_eq!(track_count(&session), count);
        }
    }
    let expected = call(&session, "project.summary");
    drop(session);
    let reopened = ApiSession::open(&fixture.0).unwrap();
    assert_eq!(call(&reopened, "project.summary"), expected);
}

#[test]
fn valid_backend_envelopes_allow_extensions_and_acknowledge_one_durable_edit() {
    for legacy in [false, true] {
        let fixture = Fixture::new();
        let session = Arc::new(ApiSession::open(&fixture.0).unwrap());
        let result = backend_response(
            json!({"jsonrpc":"2.0", "id":1, "result":{"accepted":true}, "extension":null}),
            legacy,
            Some(Arc::clone(&session)),
        );
        assert_eq!(result["isError"], false);
        assert_eq!(
            result["structuredContent"],
            json!({"result":{"accepted":true}})
        );
        let expected = call(&session, "project.summary");
        assert_eq!(track_count(&session), 1);
        call(&session, "edit.undo");
        assert_eq!(track_count(&session), 0);
        call(&session, "edit.redo");
        assert_eq!(call(&session, "project.summary"), expected);
        drop(session);
        let reopened = ApiSession::open(&fixture.0).unwrap();
        assert_eq!(call(&reopened, "project.summary"), expected);
    }
}
