use super::*;
use echode_history::write_package;
use echode_model::Project;
use std::sync::atomic::{AtomicU64, Ordering};

struct Cleanup(std::path::PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open_session() -> (Cleanup, ApiSession) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!("echode-rpc-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    write_package(&directory, &Project::new()).unwrap();
    let session = ApiSession::open(&directory).unwrap();
    (Cleanup(directory), session)
}

fn response(session: &ApiSession, line: &str) -> Value {
    serde_json::from_str(&session.reply(line).expect("response")).unwrap()
}

#[test]
fn the_catalog_lists_track_add() {
    let (_cleanup, session) = open_session();
    let body = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.list","id":1}"#,
    );
    let ids: Vec<&str> = body["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"track.add"));
    assert_eq!(body["id"], 1);
}

#[test]
fn performing_an_action_shows_up_in_the_summary() {
    let (_cleanup, session) = open_session();
    let added = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"track.add","name":"piano"},"id":"a"}"#,
    );
    let id = added["result"]["id"].as_str().unwrap();
    let body = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"project.summary","id":2}"#,
    );
    assert_eq!(body["result"]["tracks"][0]["id"], id);
    assert_eq!(body["result"]["tracks"][0]["name"], "piano");
    assert!(body["result"]["tracks"][0]["parent"].is_null());
}

#[test]
fn a_rejected_edit_leaves_the_project_unchanged() {
    let (_cleanup, session) = open_session();
    response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"track.add","name":"piano"},"id":1}"#,
    );
    let added = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"track.add","name":"piano"},"id":2}"#,
    );
    let track = added["result"]["id"].as_str().unwrap();
    let note = format!(
        r#"{{"jsonrpc":"2.0","method":"actions.perform","params":{{"type":"note.insert","track":"{track}","pitch":60,"velocity":100,"start":0,"length":705600}},"id":3}}"#
    );
    response(&session, &note);
    let rejected = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"project.set_meter","numerator":3,"denominator":4},"id":4}"#,
    );
    assert_eq!(rejected["error"]["code"], -32000);
    let body = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"project.summary","id":5}"#,
    );
    assert_eq!(body["result"]["meter"]["numerator"], 4);
    assert_eq!(body["result"]["tracks"].as_array().unwrap().len(), 2);
}

#[test]
fn the_api_can_set_a_fader() {
    let (_cleanup, session) = open_session();
    let added = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"track.add","name":"piano"},"id":1}"#,
    );
    let gain = added["result"]["gain"].as_str().unwrap();
    let line = format!(
        r#"{{"jsonrpc":"2.0","method":"actions.perform","params":{{"type":"param.set","parameter":"{gain}","value":0}},"id":2}}"#
    );
    let set = response(&session, &line);
    assert_eq!(set["result"]["type"], "param.set");
    assert!(set.get("error").is_none());
}

#[test]
fn bad_json_and_unknown_methods_are_errors() {
    let (_cleanup, session) = open_session();
    let parsed = response(&session, "not json");
    assert_eq!(parsed["error"]["code"], -32700);
    assert!(parsed["id"].is_null());
    let missing = response(&session, r#"{"jsonrpc":"2.0","method":"nope","id":7}"#);
    assert_eq!(missing["error"]["code"], -32601);
    assert!(
        session
            .reply(r#"{"jsonrpc":"2.0","method":"project.summary"}"#)
            .is_none()
    );
}

#[test]
fn a_catalog_id_performs_that_action() {
    let (_cleanup, session) = open_session();
    let listed = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.list","id":1}"#,
    );
    let add = listed["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "track.add")
        .unwrap();
    let schema = add["schema"].to_string();
    assert!(schema.contains("\"name\""), "{schema}");
    assert!(schema.contains("track.add"), "{schema}");
    let note = listed["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "note.insert")
        .unwrap();
    let note_schema = note["schema"].to_string();
    assert!(note_schema.contains("\"pitch\""), "{note_schema}");
    assert!(note_schema.contains("\"start\""), "{note_schema}");
    let id = add["id"].as_str().unwrap();
    let line = format!(
        r#"{{"jsonrpc":"2.0","method":"actions.perform","params":{{"type":"{id}","name":"piano"}},"id":2}}"#
    );
    let added = response(&session, &line);
    assert_eq!(added["result"]["type"], id);
    assert!(added.get("error").is_none(), "{added}");
}

#[test]
fn bad_parameters_keep_their_sentence() {
    let (_cleanup, session) = open_session();
    let meter = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"project.set_meter","numerator":1,"denominator":27},"id":1}"#,
    );
    assert_eq!(meter["error"]["code"], -32602);
    assert_eq!(
        meter["error"]["message"],
        "meter does not divide into ticks"
    );
    let tempo = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"project.set_tempo","at":0,"numerator":0,"denominator":1},"id":2}"#,
    );
    assert_eq!(
        tempo["error"]["message"],
        "tempo is outside the allowed range"
    );
    let id = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"note.delete","track":"nope","note":"00000000-0000-0000-0000-000000000001"},"id":3}"#,
    );
    assert_eq!(id["error"]["message"], "invalid id nope");
    let extra = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"track.add","name":"piano","extra":1},"id":4}"#,
    );
    assert_eq!(
        extra["error"]["message"],
        "parameters do not match an action"
    );
    let old = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.perform","params":{"type":"add_track","name":"piano"},"id":5}"#,
    );
    assert_eq!(old["error"]["message"], "parameters do not match an action");
}

#[test]
fn a_transaction_undoes_a_note_on_the_track_it_just_added() {
    let (cleanup, session) = open_session();
    let version = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"protocol.version","id":1}"#,
    );
    assert_eq!(version["result"]["version"], 1);
    let applied = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.transaction","params":{"actions":[
                {"type":"track.add","name":"piano"},
                {"type":"note.insert","track":{"ref":0,"field":"id"},"pitch":60,"velocity":100,"start":0,"length":352800},
                {"type":"param.set","parameter":{"ref":0,"field":"gain"},"value":0.5}
            ]},"id":2}"#,
    );
    assert!(applied.get("error").is_none(), "{applied}");
    assert_eq!(applied["result"][1]["track"], applied["result"][0]["id"]);
    assert_eq!(
        applied["result"][2]["parameter"],
        applied["result"][0]["gain"]
    );
    let summary = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"project.summary","id":3}"#,
    );
    assert_eq!(summary["result"]["tracks"][0]["clips"], 1);
    let undone = response(&session, r#"{"jsonrpc":"2.0","method":"edit.undo","id":4}"#);
    assert_eq!(undone["result"], serde_json::json!({}));
    let cleared = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"project.summary","id":5}"#,
    );
    assert!(cleared["result"]["tracks"].as_array().unwrap().is_empty());
    response(&session, r#"{"jsonrpc":"2.0","method":"edit.redo","id":6}"#);
    let restored = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"project.summary","id":7}"#,
    );
    assert_eq!(restored["result"]["tracks"][0]["clips"], 1);
    let log = std::fs::read_to_string(cleanup.0.join("journal.jsonl")).unwrap();
    let lines: Vec<echode_actions::JournalLine> = log
        .lines()
        .map(|line| echode_actions::decode_line(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].op, echode_actions::JournalOp::Edit);
    assert_eq!(lines[0].actions.len(), 3);
    assert_eq!(lines[1].op, echode_actions::JournalOp::Undo);
    assert!(lines[1].actions.is_empty());
    assert_eq!(lines[1].changes, lines[0].changes);
    assert_eq!(lines[2].op, echode_actions::JournalOp::Redo);
    assert!(lines[2].actions.is_empty());
    assert_eq!(lines[2].changes, lines[0].changes);
    assert!(!log.lines().nth(1).unwrap().contains("\"actions\""));
    assert!(!log.lines().nth(2).unwrap().contains("\"actions\""));
}

#[test]
fn a_failed_transaction_names_the_element_and_keeps_the_project() {
    let (_cleanup, session) = open_session();
    let rejected = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.transaction","params":{"actions":[
                {"type":"track.add","name":"piano"},
                {"type":"note.insert","track":"00000000-0000-0000-0000-000000000001","pitch":60,"velocity":100,"start":0,"length":352800}
            ]},"id":1}"#,
    );
    assert_eq!(rejected["error"]["code"], -32000);
    assert_eq!(rejected["error"]["message"], "track is not in the project");
    assert_eq!(rejected["error"]["data"]["index"], 1);
    let reference = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.transaction","params":{"actions":[
                {"type":"track.add","name":"piano"},
                {"type":"note.insert","track":{"ref":1,"field":"id"},"pitch":60,"velocity":100,"start":0,"length":352800}
            ]},"id":2}"#,
    );
    assert_eq!(reference["error"]["code"], -32602);
    assert_eq!(
        reference["error"]["message"],
        "action reference is out of range"
    );
    assert_eq!(reference["error"]["data"]["index"], 1);
    let shape = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.transaction","params":{},"id":3}"#,
    );
    assert_eq!(shape["error"]["code"], -32602);
    assert_eq!(
        shape["error"]["message"],
        "parameters do not match a transaction"
    );
    assert!(shape["error"].get("data").is_none());
    let empty = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"actions.transaction","params":{"actions":[]},"id":4}"#,
    );
    assert_eq!(empty["result"].as_array().unwrap().len(), 0);
    let undo = response(&session, r#"{"jsonrpc":"2.0","method":"edit.undo","id":5}"#);
    assert_eq!(undo["error"]["code"], -32000);
    assert_eq!(undo["error"]["message"], "nothing to undo");
    let summary = response(
        &session,
        r#"{"jsonrpc":"2.0","method":"project.summary","id":6}"#,
    );
    assert!(summary["result"]["tracks"].as_array().unwrap().is_empty());
}
