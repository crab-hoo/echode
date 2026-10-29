//! Reject incompatible backend versions before sending edits through MCP.

use std::fs;
use std::io::{BufRead, BufReader, Write};
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
        let path = std::env::temp_dir().join(format!("echode-mcp-version-{}", Id::generate()));
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

fn call(session: &ApiSession, method: &str, params: Value) -> Value {
    let request = json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params});
    let response: Value =
        serde_json::from_str(&session.reply(&request.to_string()).unwrap()).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn summary(session: &ApiSession) -> Value {
    call(session, "project.summary", json!({}))
}

fn tool(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":"edit", "method":"tools/call", "params":{
        "name":name, "arguments":arguments
    }})
}

fn run(
    session: &Arc<ApiSession>,
    version_response: Value,
    mut request: Value,
    legacy: bool,
) -> (Value, Vec<Value>) {
    let socket = Socket(PathBuf::from(format!(
        "/tmp/echode-mcp-version-{}.sock",
        Id::generate()
    )));
    let listener = bind_socket(&socket.0).unwrap();
    let session = Arc::clone(session);
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        // These deadlines bound this socket regression, not product work.
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
        assert_eq!(version["id"], 0);
        writeln!(stream.get_mut(), "{version_response}").unwrap();
        let mut forwarded = Vec::new();
        loop {
            line.clear();
            if stream.read_line(&mut line).unwrap() == 0 {
                break;
            }
            forwarded.push(serde_json::from_str(&line).unwrap());
            // A bypassed gate must expose the real edit, not fail in the fixture.
            writeln!(stream.get_mut(), "{}", session.reply(&line).unwrap()).unwrap();
        }
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "a rejected version must not cause an automatic reconnection"
        );
        forwarded
    });
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
    let forwarded = worker.join().unwrap();
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
    (reply["result"].clone(), forwarded)
}

#[test]
fn contract_backend_version_gate_preserves_edits_history_and_recovery() {
    let fixture = Fixture::new();
    let session = Arc::new(ApiSession::open(&fixture.0).unwrap());
    for name in ["kept", "redo"] {
        call(
            &session,
            "actions.perform",
            json!({"type":"track.add", "name":name}),
        );
    }
    let both = summary(&session);
    call(&session, "edit.undo", json!({}));
    let before = summary(&session);
    let journal = fs::read(fixture.0.join("journal.jsonl")).unwrap();
    for legacy in [false, true] {
        for result in [
            json!({"version":2}),
            json!({"version":0}),
            json!({"version":-1}),
            json!({"version":u64::MAX}),
            json!({"version":"1"}),
            json!({"version":1.0}),
            json!({"version":true}),
            json!({"version":null}),
            json!({}),
            json!(null),
            json!([1]),
        ] {
            for request in [
                tool("echode.track.add", json!({"name":"must not arrive"})),
                tool(
                    "echode.actions.transaction",
                    json!({"actions":[
                        {"type":"track.add", "name":"must not arrive"}
                    ]}),
                ),
                tool("echode.edit.undo", json!({})),
                tool("echode.edit.redo", json!({})),
            ] {
                let (reply, forwarded) = run(
                    &session,
                    json!({"jsonrpc":"2.0", "id":0, "result":result}),
                    request.clone(),
                    legacy,
                );
                assert!(
                    forwarded.is_empty(),
                    "an incompatible backend version must not receive an edit: {result}, {request} -> {forwarded:?}"
                );
                assert_eq!(reply["isError"], true);
                let message = reply["structuredContent"]["error"]["message"]
                    .as_str()
                    .unwrap();
                assert!(
                    message.contains("unsupported Echode session protocol"),
                    "{reply}"
                );
                assert_eq!(summary(&session), before);
                assert_eq!(fs::read(fixture.0.join("journal.jsonl")).unwrap(), journal);
            }
        }
    }
    call(&session, "edit.redo", json!({}));
    assert_eq!(summary(&session), both);
    call(&session, "edit.undo", json!({}));
    assert_eq!(summary(&session), before);
    call(&session, "edit.undo", json!({}));
    assert!(summary(&session)["tracks"].as_array().unwrap().is_empty());
    call(&session, "edit.redo", json!({}));
    assert_eq!(summary(&session), before);
    drop(session);
    let reopened = ApiSession::open(&fixture.0).unwrap();
    assert_eq!(summary(&reopened), before);
}

#[test]
fn supported_backend_version_allows_one_durable_edit_and_extra_fields() {
    for legacy in [false, true] {
        let fixture = Fixture::new();
        let session = Arc::new(ApiSession::open(&fixture.0).unwrap());
        let (reply, forwarded) = run(
            &session,
            json!({"jsonrpc":"2.0", "id":0, "result":{"version":1, "extension":true}}),
            tool("echode.track.add", json!({"name":"compatible"})),
            legacy,
        );
        assert_eq!(reply["isError"], false);
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0]["method"], "actions.perform");
        let expected = summary(&session);
        assert_eq!(expected["tracks"].as_array().unwrap().len(), 1);
        assert_eq!(expected["tracks"][0]["name"], "compatible");
        call(&session, "edit.undo", json!({}));
        assert!(summary(&session)["tracks"].as_array().unwrap().is_empty());
        call(&session, "edit.redo", json!({}));
        assert_eq!(summary(&session), expected);
        drop(session);
        let reopened = ApiSession::open(&fixture.0).unwrap();
        assert_eq!(summary(&reopened), expected);
    }
}

#[test]
fn contract_invalid_version_envelopes_do_not_authorize_edits() {
    let fixture = Fixture::new();
    let session = Arc::new(ApiSession::open(&fixture.0).unwrap());
    for name in ["kept", "redo"] {
        call(
            &session,
            "actions.perform",
            json!({"type":"track.add", "name":name}),
        );
    }
    let both = summary(&session);
    call(&session, "edit.undo", json!({}));
    let before = summary(&session);
    let journal = fs::read(fixture.0.join("journal.jsonl")).unwrap();
    for legacy in [false, true] {
        for response in envelopes::invalid_envelopes(0, json!({"version":1})) {
            let (reply, forwarded) = run(
                &session,
                response.clone(),
                tool("echode.track.add", json!({"name":"must not arrive"})),
                legacy,
            );
            assert!(
                forwarded.is_empty(),
                "an invalid version envelope must not authorize an edit: {response} -> {forwarded:?}"
            );
            assert_eq!(reply["isError"], true);
            let message = reply["structuredContent"]["error"]["message"]
                .as_str()
                .unwrap();
            assert!(message.contains("invalid session response"), "{reply}");
            assert_eq!(summary(&session), before);
            assert_eq!(fs::read(fixture.0.join("journal.jsonl")).unwrap(), journal);
        }
    }
    call(&session, "edit.redo", json!({}));
    assert_eq!(summary(&session), both);
    call(&session, "edit.undo", json!({}));
    assert_eq!(summary(&session), before);
    drop(session);
    let reopened = ApiSession::open(&fixture.0).unwrap();
    assert_eq!(summary(&reopened), before);
}
