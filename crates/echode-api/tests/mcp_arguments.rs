//! Local MCP tool validation must not connect, edit, or consume durable history.

use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

use echode_api::ApiSession;
use serde_json::{Value, json};

#[path = "support/mcp_client.rs"]
mod client;
#[path = "support/mcp_session.rs"]
mod session;
use client::Client;
use session::{Fixture, responses, tool};

fn client(socket: &Path, modern: bool) -> Client {
    let mut client = Client::new(socket);
    if !modern {
        let reply = client.request(json!({
            "jsonrpc":"2.0", "id":"init", "method":"initialize", "params":{
                "protocolVersion":"2025-11-25", "capabilities":{},
                "clientInfo":{"name":"regression", "version":"1"}
            }
        }));
        assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
        client.notify(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
    }
    client
}

fn request(client: &mut Client, next: &mut u64, modern: bool, params: Value) -> Value {
    let mut request = tool("unused", json!({}));
    let metadata = request["params"]["_meta"].clone();
    request["params"] = params;
    if modern {
        request["params"]["_meta"] = metadata;
    }
    *next += 1;
    request["id"] = json!(*next);
    client.request(request)
}

fn rejected_arguments() -> Vec<(Value, Option<&'static str>)> {
    let mut cases = Vec::new();
    // Even a matching type or null must be rejected, not overwritten or ignored.
    for kind in [
        json!("track.add"),
        json!("track.remove"),
        Value::Null,
        json!(true),
        json!(1),
        json!([]),
        json!({}),
    ] {
        cases.push((
            json!({"name":"echode.track.add", "arguments":{"name":"must not arrive", "type":kind}}),
            Some("the action type is selected by the tool name"),
        ));
    }
    for name in [
        "echode.edit.undo",
        "echode.edit.redo",
        "echode.project.summary",
    ] {
        for arguments in [json!({"extra":null}), json!({"actions":[]})] {
            cases.push((
                json!({"name":name, "arguments":arguments}),
                Some("this tool takes no arguments"),
            ));
        }
    }
    for extra in ["extra", "type", "_meta"] {
        let mut arguments = json!({"actions":[
            {"type":"track.add", "name":"must not arrive"},
            {"type":"track.add", "name":"neither may arrive"}
        ]});
        arguments[extra] = Value::Null;
        cases.push((
            json!({"name":"echode.actions.transaction", "arguments":arguments}),
            Some("transaction accepts only an actions array"),
        ));
    }
    for name in [
        "echode.track.add",
        "echode.actions.transaction",
        "echode.edit.undo",
        "echode.edit.redo",
        "echode.project.summary",
    ] {
        for arguments in [
            Value::Null,
            json!([]),
            json!(true),
            json!(7),
            json!("invalid"),
        ] {
            cases.push((json!({"name":name, "arguments":arguments}), None));
        }
    }
    for name in [
        None,
        Some(Value::Null),
        Some(json!(7)),
        Some(json!([])),
        Some(json!("track.add")),
        Some(json!("echode.unknown")),
        Some(json!("echode.protocol.version")),
    ] {
        let mut params = json!({"arguments":{"name":"must not arrive"}});
        if let Some(name) = name {
            params["name"] = name;
        }
        cases.push((params, None));
    }
    cases
}

fn accepted(client: &mut Client, next: &mut u64, modern: bool, params: Value) -> Value {
    let reply = request(client, next, modern, params);
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert_eq!(reply["result"].get("resultType").is_some(), modern);
    let text: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, reply["result"]["structuredContent"]);
    reply["result"]["structuredContent"]["result"].clone()
}

#[test]
fn contract_local_tool_arguments_preserve_backend_history_and_recovery() {
    for modern in [false, true] {
        let mut fixture = Fixture::new();
        for name in ["kept", "redo"] {
            fixture.call("actions.perform", json!({"type":"track.add", "name":name}));
        }
        let both = fixture.summary();
        fixture.call("edit.undo", json!({}));
        let before = fixture.summary();
        let journal = fs::read(fixture.directory.join("journal.jsonl")).unwrap();
        let mut client = client(&fixture.socket, modern);
        let mut next = 0;

        for (params, message) in rejected_arguments() {
            let reply = request(&mut client, &mut next, modern, params.clone());
            assert_eq!(
                fixture.connections.load(Ordering::Relaxed),
                0,
                "invalid local tool arguments must not reach the backend: modern={modern}, {params}"
            );
            if let Some(message) = message {
                assert!(reply.get("error").is_none(), "{reply}");
                assert_eq!(reply["result"]["isError"], true, "{reply}");
                assert_eq!(
                    reply["result"]["structuredContent"]["error"]["message"],
                    message
                );
                assert_eq!(reply["result"].get("resultType").is_some(), modern);
                let text: Value =
                    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap())
                        .unwrap();
                assert_eq!(text, reply["result"]["structuredContent"]);
            } else {
                assert_eq!(reply["error"]["code"], -32602, "{reply}");
                assert!(reply.get("result").is_none());
            }
            assert_eq!(fixture.summary(), before);
            assert_eq!(
                fs::read(fixture.directory.join("journal.jsonl")).unwrap(),
                journal
            );
        }
        // Refusals may neither consume an Undo nor invalidate a pre-existing Redo.
        fixture.call("edit.redo", json!({}));
        assert_eq!(fixture.summary(), both);
        fixture.call("edit.undo", json!({}));
        assert_eq!(fixture.summary(), before);
        fixture.call("edit.undo", json!({}));
        assert_eq!(fixture.summary()["tracks"], json!([]));
        fixture.call("edit.redo", json!({}));
        assert_eq!(fixture.summary(), before);

        // Omitted arguments mean an empty object only for tools that accept one.
        let summary = accepted(
            &mut client,
            &mut next,
            modern,
            json!({"name":"echode.project.summary"}),
        );
        assert_eq!(summary, before);
        accepted(
            &mut client,
            &mut next,
            modern,
            json!({"name":"echode.track.add", "arguments":{"name":"accepted single"}}),
        );
        let single = fixture.summary();
        assert_eq!(single["tracks"].as_array().unwrap().len(), 2);
        accepted(
            &mut client,
            &mut next,
            modern,
            json!({"name":"echode.actions.transaction", "arguments":{"actions":[
                {"type":"track.add", "name":"accepted first"},
                {"type":"track.add", "name":"accepted second"}
            ]}}),
        );
        let after = fixture.summary();
        assert_eq!(after["tracks"].as_array().unwrap().len(), 4);
        accepted(
            &mut client,
            &mut next,
            modern,
            json!({"name":"echode.edit.undo"}),
        );
        assert_eq!(fixture.summary(), single);
        accepted(
            &mut client,
            &mut next,
            modern,
            json!({"name":"echode.edit.undo"}),
        );
        assert_eq!(fixture.summary(), before);
        for expected in [&single, &after] {
            accepted(
                &mut client,
                &mut next,
                modern,
                json!({"name":"echode.edit.redo", "arguments":{}}),
            );
            assert_eq!(&fixture.summary(), expected);
        }
        assert_eq!(fixture.connections.load(Ordering::Relaxed), 7);
        client.close();
        fixture.close();
        let reopened = ApiSession::open(&fixture.directory).unwrap();
        let query = json!({"jsonrpc":"2.0", "id":1, "method":"project.summary"});
        let replies = responses(reopened.reply(&query.to_string()).unwrap().as_bytes());
        assert_eq!(replies[0]["result"], after);
    }
}
