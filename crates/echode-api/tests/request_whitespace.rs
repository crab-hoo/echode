//! JSON whitespace is narrower than Rust's Unicode whitespace classification.

use std::fs;
use std::io::{BufRead, BufReader, Write};
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
        let path = std::env::temp_dir().join(format!("echode-rpc-whitespace-{}", Id::generate()));
        write_package(&path, &Project::new()).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn call(reply: &mut impl FnMut(&str) -> Option<String>, method: &str, params: Value) -> Value {
    let request = json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params});
    let response: Value = serde_json::from_str(&reply(&request.to_string()).unwrap()).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn exercise(fixture: &Fixture, mut reply: impl FnMut(&str) -> Option<String>) -> Value {
    call(
        &mut reply,
        "actions.perform",
        json!({"type":"track.add", "name":"kept"}),
    );
    call(
        &mut reply,
        "actions.perform",
        json!({"type":"track.add", "name":"redo"}),
    );
    call(&mut reply, "edit.undo", json!({}));
    let before = call(&mut reply, "project.summary", json!({}));
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
        for has_id in [true, false] {
            let mut request = json!({"jsonrpc":"2.0", "method":method, "params":params});
            if has_id {
                request["id"] = json!("invalid");
            }
            // These include ASCII controls, non-breaking spaces and line separators.
            for whitespace in [
                '\u{b}', '\u{c}', '\u{85}', '\u{a0}', '\u{1680}', '\u{2003}', '\u{2028}',
                '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}',
            ] {
                for line in [
                    format!("{whitespace}{request}"),
                    format!("{request}{whitespace}"),
                ] {
                    let response = reply(&line);
                    assert!(
                        fs::read(fixture.0.join("journal.jsonl")).unwrap() == journal,
                        "non-JSON whitespace must not execute an edit: {line:?} -> {response:?}"
                    );
                    assert_eq!(
                        response
                            .as_deref()
                            .map(|text| serde_json::from_str::<Value>(text).unwrap()),
                        Some(json!({"jsonrpc":"2.0", "id":null,
                            "error":{"code":-32700, "message":"parse error"}})),
                        "non-JSON whitespace must be a parse error, even without an id: {line:?}"
                    );
                    assert_eq!(call(&mut reply, "project.summary", json!({})), before);
                }
            }
        }
    }
    // Invalid messages must not consume Undo, execute Redo or clear its branch.
    call(&mut reply, "edit.redo", json!({}));
    assert_eq!(
        call(&mut reply, "project.summary", json!({}))["tracks"][1]["name"],
        "redo"
    );
    call(&mut reply, "edit.undo", json!({}));
    call(&mut reply, "edit.undo", json!({}));
    assert_eq!(
        call(&mut reply, "project.summary", json!({}))["tracks"],
        json!([])
    );
    call(&mut reply, "edit.redo", json!({}));
    assert_eq!(call(&mut reply, "project.summary", json!({})), before);
    before
}

#[test]
fn contract_non_json_whitespace_preserves_edits_history_and_recovery() {
    // Both the public in-process entry and the socket must enforce JSON syntax.
    for socket in [false, true] {
        let fixture = Fixture::new();
        let session = ApiSession::open(&fixture.0).unwrap();
        let before = if socket {
            std::thread::scope(|scope| {
                let (server, client) = UnixStream::pair().unwrap();
                let worker = scope.spawn(move || serve_connection(&session, server).unwrap());
                // This deadline bounds the regression only, not project work.
                client
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                client
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut client = BufReader::new(client);
                let before = exercise(&fixture, |line| {
                    writeln!(client.get_mut(), "{line}").unwrap();
                    let mut response = String::new();
                    client.read_line(&mut response).unwrap();
                    Some(response)
                });
                client.get_mut().shutdown(Shutdown::Write).unwrap();
                drop(client);
                worker.join().unwrap();
                before
            })
        } else {
            let before = exercise(&fixture, |line| session.reply(line));
            drop(session);
            before
        };
        let reopened = ApiSession::open(&fixture.0).unwrap();
        assert_eq!(
            call(
                &mut |line| reopened.reply(line),
                "project.summary",
                json!({})
            ),
            before
        );
    }
}

#[test]
fn json_whitespace_and_unicode_inside_strings_keep_their_existing_meaning() {
    let fixture = Fixture::new();
    let session = ApiSession::open(&fixture.0).unwrap();
    for line in ["", " ", "\t\r\n "] {
        assert!(
            session.reply(line).is_none(),
            "blank JSON whitespace: {line:?}"
        );
    }
    for line in ["\u{b}", "\u{a0}", "\u{2003}\t", " \u{2028}\r\n"] {
        let response = session
            .reply(line)
            .expect("invalid whitespace must get a parse error");
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], -32700, "{line:?}");
        assert!(response["id"].is_null());
    }
    let name = "\u{a0}Track\u{2003}\u{2028}\u{3000}";
    let request = json!({"jsonrpc":"2.0", "id":"unicode", "method":"actions.perform",
        "params":{"type":"track.add", "name":name}});
    let response = session.reply(&format!(" \t\r\n{request}\r\n\t ")).unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], "unicode");
    assert_eq!(response["result"]["name"], name);
    assert_eq!(
        call(
            &mut |line| session.reply(line),
            "project.summary",
            json!({})
        )["tracks"][0]["name"],
        name
    );
    // A valid notification with legal surrounding whitespace still executes once.
    assert!(
        session
            .reply(" \t{\"jsonrpc\":\"2.0\",\"method\":\"edit.undo\"}\r\n ")
            .is_none()
    );
    assert_eq!(
        call(
            &mut |line| session.reply(line),
            "project.summary",
            json!({})
        )["tracks"],
        json!([])
    );
    drop(session);
    let reopened = ApiSession::open(&fixture.0).unwrap();
    assert_eq!(
        call(
            &mut |line| reopened.reply(line),
            "project.summary",
            json!({})
        )["tracks"],
        json!([])
    );
}
