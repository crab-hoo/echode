//! Large session replies must not turn durable edits into uncertain failures.

use std::fs;
use std::sync::atomic::Ordering;

use echode_api::{ApiSession, serve_mcp};
use serde_json::{Value, json};

#[path = "support/mcp_session.rs"]
mod session;
use session::{Fixture, responses, tool};

fn request(fixture: &Fixture, mut request: Value, legacy: bool) -> Value {
    let mut input = String::new();
    if legacy {
        request["params"].as_object_mut().unwrap().remove("_meta");
        input.push_str(&format!(
            "{}\n{}\n",
            json!({"jsonrpc":"2.0", "id":0, "method":"initialize", "params":{
                "protocolVersion":"2025-11-25", "capabilities":{},
                "clientInfo":{"name":"regression", "version":"1"}
            }}),
            json!({"jsonrpc":"2.0", "method":"notifications/initialized"})
        ));
    }
    // This bound is the transport request limit, not a project or track limit.
    assert!(request.to_string().len() < 1024 * 1024);
    input.push_str(&format!("{request}\n"));
    let mut output = Vec::new();
    serve_mcp(&fixture.socket, input.as_bytes(), &mut output).unwrap();
    let mut replies = responses(&output);
    assert_eq!(replies.len(), if legacy { 2 } else { 1 });
    let reply = replies.pop().unwrap();
    assert_eq!(reply["id"], request["id"]);
    assert!(reply.get("error").is_none(), "{}", reply["error"]);
    assert_ne!(
        reply["result"]["isError"], true,
        "{}",
        reply["result"]["structuredContent"]["error"]
    );
    reply["result"].clone()
}

#[test]
fn contract_large_mcp_replies_preserve_history_and_recovery() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new();
        let name = "n".repeat(5_060);
        // Few edits and long names cross the old reply limit without a slow edit.
        let actions = vec![json!({"type":"track.add", "name":name}); 200];
        let result = request(
            &fixture,
            tool("echode.actions.transaction", json!({"actions":actions})),
            legacy,
        );
        assert!(result["structuredContent"]["result"].to_string().len() > 1024 * 1024);
        assert_eq!(
            result["structuredContent"]["result"]
                .as_array()
                .unwrap()
                .len(),
            200
        );
        assert_eq!(fixture.call("history.usage", json!({}))["undo_steps"], 1);
        assert_eq!(
            fixture.connections.load(Ordering::Relaxed),
            1,
            "no edit retry"
        );
        let committed = fixture.summary();
        assert_eq!(committed["tracks"].as_array().unwrap().len(), 200);
        request(&fixture, tool("echode.edit.undo", json!({})), legacy);
        assert!(fixture.summary()["tracks"].as_array().unwrap().is_empty());
        request(&fixture, tool("echode.edit.redo", json!({})), legacy);
        assert_eq!(fixture.summary(), committed);

        // Also cross the old limit with a read-only result, for tools and resources.
        fixture.call(
            "actions.transaction",
            json!({"actions":vec![
                json!({"type":"track.add", "name":name}); 40
            ]}),
        );
        let expected = fixture.summary();
        assert!(expected.to_string().len() > 1024 * 1024);
        let journal = fs::read(fixture.directory.join("journal.jsonl")).unwrap();
        let summary = request(&fixture, tool("echode.project.summary", json!({})), legacy);
        assert_eq!(summary["structuredContent"]["result"], expected);
        for (uri, data) in [
            ("echode://project/summary", &expected),
            ("echode://project/tracks", &expected["tracks"]),
        ] {
            let mut resource = tool("unused", json!({}));
            resource["method"] = json!("resources/read");
            resource["params"].as_object_mut().unwrap().remove("name");
            resource["params"]
                .as_object_mut()
                .unwrap()
                .remove("arguments");
            resource["params"]["uri"] = json!(uri);
            let reply = request(&fixture, resource, legacy);
            let actual: Value =
                serde_json::from_str(reply["contents"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(&actual, data);
        }
        assert_eq!(
            fs::read(fixture.directory.join("journal.jsonl")).unwrap(),
            journal
        );
        assert_eq!(fixture.call("history.usage", json!({}))["undo_steps"], 2);
        assert_eq!(fixture.connections.load(Ordering::Relaxed), 6);
        fixture.close();
        let reopened = ApiSession::open(&fixture.directory).unwrap();
        let reply = reopened
            .reply(&json!({"jsonrpc":"2.0", "id":1, "method":"project.summary"}).to_string())
            .unwrap();
        assert_eq!(responses(reply.as_bytes())[0]["result"], expected);
    }
}
