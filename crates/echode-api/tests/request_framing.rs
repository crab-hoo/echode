//! Only newline-terminated socket frames may reach the document host.

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
        let path = std::env::temp_dir().join(format!("echode-rpc-framing-{}", Id::generate()));
        write_package(&path, &Project::new()).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})
}

fn call(session: &ApiSession, method: &str, params: Value) -> Value {
    let response: Value =
        serde_json::from_str(&session.reply(&request(method, params).to_string()).unwrap())
            .unwrap();
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn summary(session: &ApiSession) -> Value {
    call(session, "project.summary", json!({}))
}

fn exchange(session: ApiSession, chunks: &[&[u8]]) -> (ApiSession, Result<(), String>, String) {
    let (server, mut client) = UnixStream::pair().unwrap();
    // These deadlines only bound this socket regression, not product work.
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let worker = std::thread::spawn(move || {
        let result = serve_connection(&session, server).map_err(|error| error.to_string());
        (session, result)
    });
    for chunk in chunks {
        client.write_all(chunk).unwrap();
    }
    client.shutdown(Shutdown::Write).unwrap();
    let mut replies = String::new();
    let read = client.read_to_string(&mut replies);
    drop(client);
    let (session, result) = worker.join().unwrap();
    read.unwrap();
    (session, result, replies)
}

#[test]
fn contract_unterminated_socket_frames_preserve_edits_history_and_recovery() {
    let fixture = Fixture::new();
    let mut session = ApiSession::open(&fixture.0).unwrap();
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
        request(
            "actions.perform",
            json!({"type":"track.add", "name":"unframed"}),
        ),
        request(
            "actions.transaction",
            json!({"actions":[{"type":"track.add", "name":"unframed"}]}),
        ),
        request("edit.undo", json!({})),
        request("edit.redo", json!({})),
    ];
    for operation in operations {
        for notification in [false, true] {
            let mut operation = operation.clone();
            if notification {
                operation.as_object_mut().unwrap().remove("id");
            }
            let bytes = operation.to_string();
            let (returned, result, replies) = exchange(session, &[bytes.as_bytes()]);
            session = returned;
            assert_eq!(
                fs::read(fixture.0.join("journal.jsonl")).unwrap(),
                journal,
                "an unterminated socket frame must not execute an edit: {operation}"
            );
            assert_eq!(summary(&session), before);
            assert!(result.unwrap_err().contains("without a newline"));
            assert!(
                replies.is_empty(),
                "an incomplete frame must not get an RPC reply"
            );
        }
    }
    // Rejections must not clear the Redo that existed before these connections.
    call(&session, "edit.redo", json!({}));
    assert_eq!(summary(&session)["tracks"][1]["name"], "redo");
    call(&session, "edit.undo", json!({}));
    assert_eq!(summary(&session), before);
    // A valid prefix is durable, but the following incomplete Undo is not run.
    let complete = request(
        "actions.perform",
        json!({"type":"track.add", "name":"complete"}),
    );
    let incomplete = request("edit.undo", json!({}));
    let input = format!("{complete}\n{incomplete}");
    let (session, result, replies) = exchange(session, &[input.as_bytes()]);
    assert!(result.unwrap_err().contains("without a newline"));
    let responses: Vec<Value> = replies
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 1);
    assert!(responses[0].get("result").is_some());
    assert_eq!(summary(&session)["tracks"][1]["name"], "complete");
    call(&session, "edit.undo", json!({}));
    assert_eq!(summary(&session), before);
    call(&session, "edit.undo", json!({}));
    assert_eq!(summary(&session)["tracks"], json!([]));
    call(&session, "edit.redo", json!({}));
    assert_eq!(summary(&session), before);
    call(&session, "edit.redo", json!({}));
    let expected = summary(&session);
    assert_eq!(expected["tracks"][1]["name"], "complete");
    drop(session);
    assert_eq!(summary(&ApiSession::open(&fixture.0).unwrap()), expected);
}

#[test]
fn fragmented_crlf_and_empty_connections_keep_their_framing() {
    let fixture = Fixture::new();
    let session = ApiSession::open(&fixture.0).unwrap();
    let bytes = request(
        "actions.perform",
        json!({"type":"track.add", "name":"fragmented"}),
    )
    .to_string();
    let (session, result, replies) = exchange(
        session,
        &[
            &bytes.as_bytes()[..10],
            &bytes.as_bytes()[10..],
            b"\r",
            b"\n",
        ],
    );
    result.unwrap();
    assert_eq!(replies.lines().count(), 1);
    assert_eq!(summary(&session)["tracks"][0]["name"], "fragmented");
    let (session, result, replies) = exchange(session, &[]);
    result.unwrap();
    assert!(replies.is_empty());
    let (mut session, result, replies) = exchange(session, &[b"\n\r\n"]);
    result.unwrap();
    assert!(replies.is_empty());
    for bytes in [b"{".as_slice(), b" ", b"\r"] {
        let (returned, result, replies) = exchange(session, &[bytes]);
        assert!(result.unwrap_err().contains("without a newline"));
        assert!(replies.is_empty());
        assert_eq!(summary(&returned)["tracks"][0]["name"], "fragmented");
        session = returned;
    }
}
