//! Modern request metadata must guard edits independently of legacy readiness.

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

const VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

fn metadata() -> Value {
    json!({VERSION:"2026-07-28", CAPABILITIES:{}})
}

fn invalid_metadata() -> Vec<(Value, i32)> {
    let mut cases = Vec::new();
    for version in [
        json!("2099-01-01"),
        json!("2025-11-25"),
        json!(""),
        Value::Null,
        json!(true),
        json!(7),
        json!(1.0),
        json!([]),
        json!({}),
    ] {
        let code = if version.is_string() { -32022 } else { -32602 };
        let mut meta = metadata();
        meta[VERSION] = version;
        cases.push((meta, code));
    }
    for capabilities in [
        None,
        Some(Value::Null),
        Some(json!(true)),
        Some(json!(0)),
        Some(json!(1.0)),
        Some(json!("invalid")),
        Some(json!([])),
    ] {
        let mut meta = metadata();
        match capabilities {
            Some(value) => meta[CAPABILITIES] = value,
            None => {
                meta.as_object_mut().unwrap().remove(CAPABILITIES);
            }
        }
        cases.push((meta, -32602));
    }
    cases
}

fn operations() -> Vec<Value> {
    vec![
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
}

fn list(client: &mut Client) {
    let reply = client.request(json!({
        "jsonrpc":"2.0", "id":7, "method":"tools/list", "params":{"_meta":metadata()}
    }));
    assert!(reply["result"]["tools"].is_array(), "{reply}");
    assert_eq!(reply["result"]["resultType"], "complete");
    assert_eq!(reply["result"]["ttlMs"], 0);
    assert_eq!(reply["result"]["cacheScope"], "private");
}

fn accepted(client: &mut Client, name: &str, arguments: Value) {
    let mut request = tool(name, arguments);
    // Additional metadata and object-valued capabilities must remain extensible.
    request["params"]["_meta"]["trace"] = json!({"label":"日本語 / 🦀"});
    request["params"]["_meta"][CAPABILITIES] = json!({"experimental":{"future":{}}});
    let reply = client.request(request);
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert_eq!(reply["result"]["resultType"], "complete");
    assert_eq!(
        reply["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "echode"
    );
}

fn assert_reopened(fixture: &mut Fixture, expected: &Value) {
    fixture.close();
    let reopened = ApiSession::open(&fixture.directory).unwrap();
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"project.summary"});
    let replies = responses(reopened.reply(&request.to_string()).unwrap().as_bytes());
    assert_eq!(&replies[0]["result"], expected);
}

#[test]
fn contract_modern_metadata_guards_edits_without_inheriting_legacy_readiness() {
    for legacy_ready in [false, true] {
        let mut fixture = Fixture::new();
        for name in ["kept", "redo"] {
            fixture.call("actions.perform", json!({"type":"track.add", "name":name}));
        }
        let both = fixture.summary();
        fixture.call("edit.undo", json!({}));
        let before = fixture.summary();
        let journal = fs::read(fixture.directory.join("journal.jsonl")).unwrap();
        let mut client = Client::new(&fixture.socket);
        list(&mut client);
        if legacy_ready {
            let reply = client.request(json!({
                "jsonrpc":"2.0", "id":7, "method":"initialize", "params":{
                    "protocolVersion":"2025-11-25", "capabilities":{},
                    "clientInfo":{"name":"regression", "version":"1"}
                }
            }));
            assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
            client.notify(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
        }
        for (meta, code) in invalid_metadata() {
            for mut request in operations() {
                request["params"]["_meta"] = meta.clone();
                let reply = client.request(request.clone());
                assert_eq!(
                    fixture.connections.load(Ordering::Relaxed),
                    0,
                    "invalid modern metadata must block backend edits: legacy_ready={legacy_ready}, {request}"
                );
                assert_eq!(reply["error"]["code"], code, "{reply}");
                assert!(reply.get("result").is_none());
                if code == -32022 {
                    assert_eq!(reply["error"]["data"]["requested"], meta[VERSION]);
                    assert_eq!(
                        reply["error"]["data"]["supported"],
                        json!(["2026-07-28", "2025-11-25"])
                    );
                }
                assert_eq!(fixture.summary(), before);
                assert_eq!(
                    fs::read(fixture.directory.join("journal.jsonl")).unwrap(),
                    journal
                );
            }
        }
        list(&mut client);
        // Rejections may neither consume an Undo nor invalidate the existing Redo.
        fixture.call("edit.redo", json!({}));
        assert_eq!(fixture.summary(), both);
        fixture.call("edit.undo", json!({}));
        assert_eq!(fixture.summary(), before);
        fixture.call("edit.undo", json!({}));
        assert_eq!(fixture.summary()["tracks"], json!([]));
        fixture.call("edit.redo", json!({}));
        assert_eq!(fixture.summary(), before);

        accepted(
            &mut client,
            "echode.actions.transaction",
            json!({"actions":[
                {"type":"track.add", "name":"accepted first"},
                {"type":"track.add", "name":"accepted second"}
            ]}),
        );
        let batch = fixture.summary();
        assert_eq!(batch["tracks"].as_array().unwrap().len(), 3);
        accepted(
            &mut client,
            "echode.track.add",
            json!({"name":"accepted third"}),
        );
        let after = fixture.summary();
        assert_eq!(after["tracks"].as_array().unwrap().len(), 4);
        accepted(&mut client, "echode.edit.undo", json!({}));
        assert_eq!(fixture.summary(), batch);
        accepted(&mut client, "echode.edit.undo", json!({}));
        assert_eq!(fixture.summary(), before);
        accepted(&mut client, "echode.edit.redo", json!({}));
        assert_eq!(fixture.summary(), batch);
        accepted(&mut client, "echode.edit.redo", json!({}));
        assert_eq!(fixture.summary(), after);
        assert_eq!(fixture.connections.load(Ordering::Relaxed), 6);
        client.close();
        assert_reopened(&mut fixture, &after);
    }
}

#[test]
fn modern_metadata_is_not_reused_by_later_requests_or_legacy_notifications() {
    let mut fixture = Fixture::new();
    fixture.call(
        "actions.perform",
        json!({"type":"track.add", "name":"kept"}),
    );
    let before = fixture.summary();
    let journal = fs::read(fixture.directory.join("journal.jsonl")).unwrap();
    let mut client = Client::new(&fixture.socket);
    list(&mut client);
    client.notify(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
    for meta in [
        None,
        Some(Value::Null),
        Some(json!([])),
        Some(json!(true)),
        Some(json!("invalid")),
        Some(json!({})),
        Some(json!({CAPABILITIES:{}})),
    ] {
        for mut request in operations() {
            match &meta {
                Some(meta) => request["params"]["_meta"] = meta.clone(),
                None => {
                    request["params"].as_object_mut().unwrap().remove("_meta");
                }
            }
            let reply = client.request(request);
            assert_eq!(reply["error"]["code"], -32602, "{reply}");
            assert!(reply.get("result").is_none());
            assert_eq!(fixture.connections.load(Ordering::Relaxed), 0);
            assert_eq!(fixture.summary(), before);
            assert_eq!(
                fs::read(fixture.directory.join("journal.jsonl")).unwrap(),
                journal
            );
        }
    }
    // A valid later modern request still works without initializing the legacy stream.
    accepted(&mut client, "echode.track.add", json!({"name":"accepted"}));
    let after = fixture.summary();
    assert_eq!(after["tracks"].as_array().unwrap().len(), 2);
    client.close();
    assert_reopened(&mut fixture, &after);
}
