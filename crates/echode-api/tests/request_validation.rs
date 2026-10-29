//! Validate the RPC envelope before an edit can reach the document host.

use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use echode_api::{ApiSession, serve_connection};
use echode_history::write_package;
use echode_model::{Id, Project};
use serde_json::{Value, json};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("echode-rpc-envelope-{}", Id::generate()));
        write_package(&path, &Project::new()).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn reply(session: &ApiSession, request: &Value) -> Value {
    serde_json::from_str(&session.reply(&request.to_string()).expect("RPC response")).unwrap()
}

fn call(session: &ApiSession, method: &str, params: Value) -> Value {
    let result = reply(
        session,
        &json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params}),
    );
    assert!(result.get("error").is_none(), "{result}");
    result["result"].clone()
}

fn summary(session: &ApiSession) -> Value {
    call(session, "project.summary", json!({}))
}

#[test]
fn contract_invalid_rpc_envelopes_preserve_edits_history_and_recovery() {
    let fixture = Fixture::new();
    let session = ApiSession::open(&fixture.0).unwrap();
    call(
        &session,
        "actions.perform",
        json!({"type":"track.add", "name":"kept"}),
    );
    call(
        &session,
        "actions.perform",
        json!({"type":"track.add", "name":"redo"}),
    );
    call(&session, "edit.undo", json!({}));
    let before = summary(&session);
    let journal = fs::read(fixture.0.join("journal.jsonl")).unwrap();
    let operations = [
        (
            "actions.perform",
            json!({"type":"track.add", "name":"invalid"}),
        ),
        (
            "actions.transaction",
            json!({"actions":[{"type":"track.add", "name":"invalid"}]}),
        ),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
    ];
    for (method, params) in operations {
        let base = json!({"jsonrpc":"2.0", "id":"request", "method":method, "params":params});
        let mut invalid = Vec::new();
        for id in [
            json!(true),
            json!(false),
            json!([]),
            json!([1]),
            json!({"id":1}),
        ] {
            let mut request = base.clone();
            request["id"] = id;
            invalid.push((request, Value::Null));
        }
        for version in [json!(null), json!(1), json!("1.0")] {
            let mut request = base.clone();
            request["jsonrpc"] = version;
            invalid.push((request, json!("request")));
        }
        let mut missing_version = base.clone();
        missing_version.as_object_mut().unwrap().remove("jsonrpc");
        invalid.push((missing_version, json!("request")));
        for params in [json!(null), json!(true), json!(0), json!("ignored")] {
            let mut request = base.clone();
            request["params"] = params;
            invalid.push((request, json!("request")));
        }
        for (request, id) in invalid {
            let response = reply(&session, &request);
            assert!(
                fs::read(fixture.0.join("journal.jsonl")).unwrap() == journal,
                "invalid RPC envelope must not execute an edit: {request} -> {response}"
            );
            assert_eq!(summary(&session), before, "{request}");
            assert_eq!(
                response,
                json!({"jsonrpc":"2.0", "id":id,
                    "error":{"code":-32600, "message":"invalid request"}}),
                "invalid RPC envelope must be rejected: {request}"
            );
        }
    }
    // Rejections must neither consume Undo nor clear or execute the pending Redo.
    call(&session, "edit.redo", json!({}));
    assert_eq!(summary(&session)["tracks"][1]["name"], "redo");
    call(&session, "edit.undo", json!({}));
    assert_eq!(summary(&session), before);
    call(&session, "edit.undo", json!({}));
    assert_eq!(summary(&session)["tracks"], json!([]));
    call(&session, "edit.redo", json!({}));
    assert_eq!(summary(&session), before);
    drop(session);
    let reopened = ApiSession::open(&fixture.0).unwrap();
    assert_eq!(summary(&reopened), before);
}

#[test]
fn contract_socket_requests_distinguish_invalid_envelopes_from_notifications() {
    let fixture = Fixture::new();
    let session = ApiSession::open(&fixture.0).unwrap();
    let (server, mut client) = UnixStream::pair().unwrap();
    // These deadlines only bound this socket regression, not product work.
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let worker = std::thread::spawn(move || {
        serve_connection(&session, server).unwrap();
        session
    });
    let invalid = [
        json!({}),
        json!({"jsonrpc":"2.0"}),
        json!({"jsonrpc":"2.0", "method":false}),
        json!({"jsonrpc":"2.0", "method":null, "id":"known"}),
        json!({"method":"actions.perform", "params":{"type":"track.add", "name":"bad"}}),
        json!({"jsonrpc":"1.0", "method":"edit.undo"}),
        json!({"jsonrpc":"2.0", "method":"edit.redo", "params":null}),
        json!(null),
        json!([]),
        json!([{"jsonrpc":"2.0", "method":"edit.undo", "id":1}]),
        json!(42),
        json!("request"),
    ];
    for request in &invalid {
        writeln!(client, "{request}").unwrap();
    }
    // Only valid notifications suppress replies, including method/argument errors.
    for notification in [
        json!({"jsonrpc":"2.0", "method":"actions.perform", "params":{"type":"track.add", "name":"once"}}),
        json!({"jsonrpc":"2.0", "method":"unknown"}),
        json!({"jsonrpc":"2.0", "method":"actions.perform", "params":[]}),
    ] {
        writeln!(client, "{notification}").unwrap();
    }
    let ids = [
        json!(null),
        json!(""),
        json!("request"),
        json!(0),
        json!(-1),
        json!(1.5),
    ];
    for id in &ids {
        writeln!(
            client,
            "{}",
            json!({"jsonrpc":"2.0", "method":"protocol.version", "id":id})
        )
        .unwrap();
    }
    writeln!(
        client,
        "{}",
        json!({"jsonrpc":"2.0", "method":"actions.perform", "params":[], "id":"params"})
    )
    .unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    let mut text = String::new();
    let read = client.read_to_string(&mut text);
    drop(client);
    let session = worker.join().unwrap();
    read.unwrap();
    let responses: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), invalid.len() + ids.len() + 1, "{text}");
    for (request, response) in invalid.iter().zip(&responses) {
        assert_eq!(response["error"]["code"], -32600, "{request}: {response}");
        assert_eq!(
            response["id"],
            request.get("id").cloned().unwrap_or(Value::Null)
        );
    }
    for (id, response) in ids.iter().zip(&responses[invalid.len()..]) {
        assert_eq!(response["id"], *id);
        assert_eq!(response["result"], json!({"version":1}));
    }
    let last = responses.last().unwrap();
    assert_eq!(last["id"], "params");
    assert_eq!(last["error"]["code"], -32602);
    let tracks = summary(&session)["tracks"].as_array().unwrap().clone();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["name"], "once");
    call(&session, "edit.undo", json!({}));
    assert_eq!(summary(&session)["tracks"], json!([]));
}
