//! Legacy lifecycle and replay guards must protect the real document history.

use std::fs;
use std::sync::atomic::Ordering;

use echode_api::ApiSession;
use serde_json::{Value, json};

#[path = "support/mcp_client.rs"]
mod client;
#[path = "support/mcp_session.rs"]
mod session;
use client::Client;
use session::{Fixture, responses, tool};

fn initialize(id: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "method":"initialize", "params":{
        "protocolVersion":"2025-11-25", "capabilities":{},
        "clientInfo":{"name":"regression", "version":"1"}
    }})
}

fn operations(id: &Value) -> Vec<Value> {
    [
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
    ]
    .into_iter()
    .map(|mut request| {
        request["id"] = id.clone();
        request["params"].as_object_mut().unwrap().remove("_meta");
        request
    })
    .collect()
}

fn assert_blocked(
    client: &mut Client,
    fixture: &Fixture,
    id: Value,
    code: i32,
    connections: usize,
) {
    let before = fixture.summary();
    let journal = fs::read(fixture.directory.join("journal.jsonl")).unwrap();
    for request in operations(&id) {
        let reply = client.request(request.clone());
        assert_eq!(
            fixture.connections.load(Ordering::Relaxed),
            connections,
            "legacy lifecycle and replay guards must block backend edits: {request}"
        );
        assert_eq!(reply["error"]["code"], code, "{reply}");
        assert!(reply.get("result").is_none());
        assert_eq!(fixture.summary(), before);
        assert_eq!(
            fs::read(fixture.directory.join("journal.jsonl")).unwrap(),
            journal
        );
    }
}

fn assert_reopened(fixture: &mut Fixture, expected: &Value) {
    fixture.close();
    let reopened = ApiSession::open(&fixture.directory).unwrap();
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"project.summary"});
    let replies = responses(reopened.reply(&request.to_string()).unwrap().as_bytes());
    assert_eq!(&replies[0]["result"], expected);
}

#[test]
fn contract_legacy_lifecycle_and_replay_guards_preserve_durable_history() {
    let mut fixture = Fixture::new();
    for name in ["kept", "redo"] {
        fixture.call("actions.perform", json!({"type":"track.add", "name":name}));
    }
    let both = fixture.summary();
    fixture.call("edit.undo", json!({}));
    let before = fixture.summary();
    let mut client = Client::new(&fixture.socket);

    // An initialized notification cannot skip the initialize request.
    client.notify(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
    assert_blocked(&mut client, &fixture, json!("fresh"), -32602, 0);

    for params in [
        json!({}),
        json!({"protocolVersion":1, "capabilities":{}, "clientInfo":{"name":"x", "version":"1"}}),
        json!({"protocolVersion":"2025-11-25", "capabilities":[], "clientInfo":{"name":"x", "version":"1"}}),
        json!({"protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"x"}}),
    ] {
        let mut request = initialize(json!("invalid-init"));
        request["params"] = params;
        assert_eq!(client.request(request)["error"]["code"], -32602);
    }
    let reply = client.request(initialize(json!("init")));
    assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
    assert_blocked(&mut client, &fixture, json!("pending"), -32602, 0);

    // Malformed notification params and a request-shaped notification cannot enable edits.
    for params in [Value::Null, json!([]), json!(true), json!("invalid")] {
        client.notify(json!({
            "jsonrpc":"2.0", "method":"notifications/initialized", "params":params
        }));
        assert_blocked(&mut client, &fixture, json!("pending"), -32602, 0);
    }
    let reply = client.request(json!({
        "jsonrpc":"2.0", "id":"not-a-notification", "method":"notifications/initialized"
    }));
    assert_eq!(reply["error"]["code"], -32602);
    assert_eq!(
        client.request(initialize(json!("again")))["error"]["code"],
        -32600
    );
    assert_blocked(&mut client, &fixture, json!("pending"), -32602, 0);

    client.notify(json!({"jsonrpc":"2.0", "method":"notifications/initialized", "params":{}}));
    // IDs consumed by initialize and read-only requests cannot be replayed as edits.
    assert_blocked(&mut client, &fixture, json!("init"), -32600, 0);
    let listed = client.request(json!({"jsonrpc":"2.0", "id":"listed", "method":"tools/list"}));
    assert!(listed["result"]["tools"].is_array());
    assert_blocked(&mut client, &fixture, json!("listed"), -32600, 0);
    assert_eq!(fixture.summary(), before);

    // No rejected edit may consume an Undo or invalidate the existing Redo.
    fixture.call("edit.redo", json!({}));
    assert_eq!(fixture.summary(), both);
    fixture.call("edit.undo", json!({}));
    assert_eq!(fixture.summary(), before);
    fixture.call("edit.undo", json!({}));
    assert_eq!(fixture.summary()["tracks"], json!([]));
    fixture.call("edit.redo", json!({}));
    assert_eq!(fixture.summary(), before);

    let mut request = operations(&json!("committed")).remove(1);
    request["params"]["arguments"]["actions"] = json!([
        {"type":"track.add", "name":"accepted first"},
        {"type":"track.add", "name":"accepted second"}
    ]);
    assert_eq!(client.request(request)["result"]["isError"], false);
    let after = fixture.summary();
    assert_eq!(after["tracks"].as_array().unwrap().len(), 3);
    assert_blocked(&mut client, &fixture, json!("committed"), -32600, 1);
    // Reinitialization is refused, but does not disable the already ready stream.
    assert_eq!(
        client.request(initialize(json!("again")))["error"]["code"],
        -32600
    );
    let undo = operations(&json!("undo")).remove(2);
    assert_eq!(client.request(undo)["result"]["isError"], false);
    assert_eq!(fixture.summary(), before);
    let redo = operations(&json!("redo")).remove(3);
    assert_eq!(client.request(redo)["result"]["isError"], false);
    assert_eq!(fixture.summary(), after);
    client.close();
    assert_reopened(&mut fixture, &after);
}

#[test]
fn legacy_id_reuse_is_type_sensitive_and_failed_dispatches_still_consume_ids() {
    let mut fixture = Fixture::new();
    fixture.call(
        "actions.perform",
        json!({"type":"track.add", "name":"kept"}),
    );
    let before = fixture.summary();
    let mut client = Client::new(&fixture.socket);
    assert!(
        client
            .request(initialize(json!("init")))
            .get("error")
            .is_none()
    );
    client.notify(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
    for id in [
        json!(1),
        json!("1"),
        json!(""),
        json!("日本語 / 🦀"),
        json!(i64::MIN),
        json!(u64::MAX),
    ] {
        let reply = client.request(json!({"jsonrpc":"2.0", "id":id, "method":"tools/list"}));
        assert!(reply["result"]["tools"].is_array(), "{reply}");
        assert_blocked(&mut client, &fixture, id.clone(), -32600, 0);
    }
    let reply = client.request(json!({"jsonrpc":"2.0", "id":"unknown", "method":"missing-method"}));
    assert_eq!(reply["error"]["code"], -32601);
    assert_blocked(&mut client, &fixture, json!("unknown"), -32600, 0);
    let mut invalid = operations(&json!("invalid-tool")).remove(0);
    invalid["params"]["arguments"] = json!({"type":"track.remove"});
    assert_eq!(client.request(invalid)["result"]["isError"], true);
    assert_blocked(&mut client, &fixture, json!("invalid-tool"), -32600, 0);
    client.close();
    assert_reopened(&mut fixture, &before);
}
