//! MCP request IDs and notifications must guard the durable backend, not just replies.

use std::fs;
use std::sync::atomic::Ordering;

use echode_api::{ApiSession, serve_mcp};
use serde_json::{Value, json};

#[path = "support/mcp_session.rs"]
mod session;
use session::{Fixture, responses, tool};

fn run(fixture: &Fixture, mut request: Value, legacy: bool) -> Vec<Value> {
    let prefix = if legacy {
        request["params"].as_object_mut().unwrap().remove("_meta");
        concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":\"init\",\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"regression\",\"version\":\"1\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n"
        )
    } else {
        ""
    };
    let input = format!(
        "{prefix}{request}\n{{\"jsonrpc\":\"2.0\",\"id\":\"after\",\"method\":\"ping\"}}\n"
    );
    let mut output = Vec::new();
    serve_mcp(&fixture.socket, input.as_bytes(), &mut output).unwrap();
    let mut replies = responses(&output);
    if legacy {
        let initialized = replies.remove(0);
        assert_eq!(initialized["id"], "init");
        assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    }
    let following = replies.pop().unwrap();
    assert_eq!(
        following["id"], "after",
        "a later request must remain usable"
    );
    assert_eq!(following["result"], json!({}));
    replies
}

fn assert_reopened(fixture: &mut Fixture, expected: &Value) {
    fixture.close();
    let reopened = ApiSession::open(&fixture.directory).unwrap();
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"project.summary"});
    let response: Value =
        serde_json::from_str(&reopened.reply(&request.to_string()).unwrap()).unwrap();
    assert_eq!(&response["result"], expected);
}

#[test]
fn contract_invalid_mcp_ids_and_notifications_preserve_edits_history_and_recovery() {
    let mut fixture = Fixture::new();
    for name in ["kept", "redo"] {
        fixture.call("actions.perform", json!({"type":"track.add", "name":name}));
    }
    let both = fixture.summary();
    fixture.call("edit.undo", json!({}));
    let before = fixture.summary();
    let journal = fs::read(fixture.directory.join("journal.jsonl")).unwrap();
    for legacy in [false, true] {
        for operation in [
            tool("echode.track.add", json!({"name":"must not arrive"})),
            tool(
                "echode.actions.transaction",
                json!({"actions":[
                    {"type":"track.add", "name":"must not arrive"},
                    {"type":"track.add", "name":"neither may arrive"}
                ]}),
            ),
            tool("echode.edit.undo", json!({})),
            tool("echode.edit.redo", json!({})),
        ] {
            // None omits the ID (a notification); Some(Null) is an invalid MCP ID.
            for id in [
                Some(Value::Null),
                Some(json!(true)),
                Some(json!(false)),
                Some(json!([])),
                Some(json!({})),
                Some(json!(1.5)),
                Some(json!(1.0)),
                None,
            ] {
                let mut request = operation.clone();
                match &id {
                    Some(id) => request["id"] = id.clone(),
                    None => {
                        request.as_object_mut().unwrap().remove("id");
                    }
                }
                let replies = run(&fixture, request.clone(), legacy);
                assert_eq!(
                    fixture.connections.load(Ordering::Relaxed),
                    0,
                    "invalid MCP IDs and notifications must not reach the backend: {request}"
                );
                assert_eq!(fixture.summary(), before);
                assert_eq!(
                    fs::read(fixture.directory.join("journal.jsonl")).unwrap(),
                    journal
                );
                if id.is_some() {
                    assert_eq!(replies.len(), 1);
                    assert_eq!(replies[0]["jsonrpc"], "2.0");
                    assert_eq!(replies[0]["id"], Value::Null);
                    assert_eq!(replies[0]["error"]["code"], -32600);
                    assert!(replies[0].get("result").is_none());
                } else {
                    assert!(replies.is_empty(), "notifications must not produce replies");
                }
            }
        }
    }
    // Rejections must neither consume an Undo nor invalidate the existing Redo.
    fixture.call("edit.redo", json!({}));
    assert_eq!(fixture.summary(), both);
    fixture.call("edit.undo", json!({}));
    assert_eq!(fixture.summary(), before);
    fixture.call("edit.undo", json!({}));
    assert_eq!(fixture.summary()["tracks"], json!([]));
    fixture.call("edit.redo", json!({}));
    assert_eq!(fixture.summary(), before);
    assert_reopened(&mut fixture, &before);
}

#[test]
fn valid_string_and_integer_ids_keep_their_types_and_allow_durable_edits() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new();
        let mut expected = fixture.summary();
        for (index, id) in [
            json!(""),
            json!("日本語 / 🦀"),
            json!(0),
            json!(-1),
            json!(i64::MIN),
            json!(u64::MAX),
        ]
        .into_iter()
        .enumerate()
        {
            let mut request = tool("echode.track.add", json!({"name":"accepted"}));
            request["id"] = id.clone();
            request["extension"] = json!({"future":true});
            let replies = run(&fixture, request, legacy);
            assert_eq!(replies.len(), 1);
            assert_eq!(replies[0]["id"], id);
            assert_eq!(replies[0]["result"]["isError"], false);
            assert_eq!(fixture.connections.load(Ordering::Relaxed), index + 1);
            let after = fixture.summary();
            assert_eq!(after["tracks"].as_array().unwrap().len(), index + 1);
            fixture.call("edit.undo", json!({}));
            assert_eq!(fixture.summary(), expected);
            fixture.call("edit.redo", json!({}));
            assert_eq!(fixture.summary(), after);
            expected = after;
        }
        assert_reopened(&mut fixture, &expected);
    }
}
