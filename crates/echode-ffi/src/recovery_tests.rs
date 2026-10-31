//! Recovery consent through the product C ABI, without hardware or windows.
use super::*;
use echode_actions::{Command, Session};
use echode_history::{AUTOSAVE_REF, Autosave, compact_journal, write_package};
use echode_model::{Id, Project};
use std::ffi::{CStr, CString};
use std::sync::{Arc, mpsc};

struct Reply(i32, Option<String>, *mut Snapshot);

extern "C" fn complete(
    context: *mut c_void,
    status: i32,
    message: *const c_char,
    snapshot: *mut Snapshot,
) {
    // The callback owns its context: the waiter may return while this runs.
    let sender = unsafe { Box::from_raw(context.cast::<mpsc::Sender<Reply>>()) };
    let message = (!message.is_null()).then(|| {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    });
    sender.send(Reply(status, message, snapshot)).unwrap();
}

fn request(document: *mut Document, operation: u32, json: Option<&str>) -> Reply {
    let (sender, incoming) = mpsc::channel::<Reply>();
    let context = Box::into_raw(Box::new(sender)).cast::<c_void>();
    unsafe {
        if let Some(json) = json {
            let entry = match operation {
                0 => echode_document_submit,
                1 => echode_document_history,
                _ => echode_document_audio,
            };
            entry(document, json.as_ptr(), json.len(), context, complete);
        } else {
            echode_document_query(document, operation, context, complete);
        }
    }
    incoming
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
}

fn report(reply: Reply) -> serde_json::Value {
    assert_eq!(reply.0, ECHODE_OK, "{:?}", reply.1);
    let mut len = 0;
    let bytes = unsafe { echode_snapshot_history(reply.2, &mut len) };
    let value = serde_json::from_slice(unsafe { std::slice::from_raw_parts(bytes, len) }).unwrap();
    unsafe { echode_snapshot_free(reply.2) };
    value
}

#[test]
fn contract_recovery_requires_consent_before_edits_and_preserves_the_original_on_cancel() {
    let path = std::env::temp_dir().join(format!("echode-recovery-{}", Id::generate()));
    write_package(&path, &Project::new()).unwrap();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack { name: "A".into() })
        .unwrap();
    let autosave = Autosave::create(&path).unwrap();
    let base = autosave
        .save(Arc::new(session.project().clone()), 1)
        .unwrap()
        .wait()
        .unwrap();
    session
        .perform(Command::AddTrack { name: "B".into() })
        .unwrap();
    let tip = autosave
        .save(Arc::new(session.project().clone()), 2)
        .unwrap()
        .wait()
        .unwrap();
    drop(autosave);
    compact_journal(&path, 2).unwrap();
    let hex = tip.to_string();
    std::fs::remove_file(path.join(".git/objects").join(&hex[..2]).join(&hex[2..])).unwrap();
    let original = std::fs::read(path.join("project.json")).unwrap();
    let journal = std::fs::read(path.join("journal.jsonl")).unwrap();
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    let document = unsafe { echode_document_open(c_path.as_ptr()) };
    assert!(!document.is_null());
    let initial = report(request(document, ECHODE_DOCUMENT_SNAPSHOT, None));
    assert_eq!(
        initial["recovery"]["tip"], hex,
        "recovery fallback must be reported before editing"
    );
    assert_eq!(initial["recovery"]["base"], base.to_string());
    let rejected = request(
        document,
        0,
        Some(r#"{"type":"track.add","name":"unconfirmed"}"#),
    );
    assert_eq!(
        rejected.0, ECHODE_ERR,
        "unconfirmed recovery must reject edits"
    );
    assert!(
        rejected
            .1
            .unwrap()
            .contains("recovery confirmation required")
    );
    assert!(rejected.2.is_null());
    for operation in [ECHODE_DOCUMENT_UNDO, ECHODE_DOCUMENT_REDO] {
        let rejected = request(document, operation, None);
        assert_eq!(rejected.0, ECHODE_ERR);
        assert!(
            rejected
                .1
                .unwrap()
                .contains("recovery confirmation required")
        );
    }
    let rejected = request(
        document,
        2,
        Some(r#"{"type":"transport.set_start","tick":705600}"#),
    );
    assert_eq!(
        rejected.0, ECHODE_ERR,
        "session writes must wait for consent too"
    );
    assert!(!path.join("transport.json").exists());
    unsafe { echode_document_close(document) };
    assert_eq!(std::fs::read(path.join("project.json")).unwrap(), original);
    assert_eq!(std::fs::read(path.join("journal.jsonl")).unwrap(), journal);
    let repo = gix_tip(&path);
    assert_eq!(repo, hex);
    let host = echode_history::DocumentHost::open(&path).unwrap();
    assert!(host.recovery_notice().is_some());
    assert!(
        host.perform(Command::AddTrack {
            name: "blocked".into()
        })
        .is_err()
    );
    assert!(
        host.apply(vec![
            serde_json::json!({"type":"track.add","name":"blocked"})
        ])
        .is_err()
    );
    assert!(host.undo().is_err());
    assert!(host.redo().is_err());
    assert!(
        host.commit_plugin_state(Id::generate(), b"blocked")
            .is_err()
    );
    drop(host);

    let document = unsafe { echode_document_open(c_path.as_ptr()) };
    let stale = request(
        document,
        1,
        Some(&format!(
            r#"{{"type":"history.accept_recovery","tip":"wrong","base":"{base}"}}"#
        )),
    );
    assert_eq!(
        stale.0, ECHODE_ERR,
        "another document's consent must not unlock this one"
    );
    assert!(report(request(document, ECHODE_DOCUMENT_SNAPSHOT, None))["recovery"].is_object());
    let accept = format!(r#"{{"type":"history.accept_recovery","tip":"{tip}","base":"{base}"}}"#);
    // A failed checkpoint must not enable editing.
    std::fs::create_dir(path.join("save.pending")).unwrap();
    assert_eq!(request(document, 1, Some(&accept)).0, ECHODE_ERR);
    assert_eq!(
        request(
            document,
            0,
            Some(r#"{"type":"track.add","name":"blocked"}"#)
        )
        .0,
        ECHODE_ERR
    );
    std::fs::remove_dir(path.join("save.pending")).unwrap();
    assert!(report(request(document, 1, Some(&accept)))["recovery"].is_null());
    let checkpoint = gix_tip(&path);
    assert_ne!(
        checkpoint, hex,
        "consent must checkpoint before accepting edits"
    );
    assert!(report(request(document, 1, Some(&accept)))["recovery"].is_null());
    assert_eq!(
        gix_tip(&path),
        checkpoint,
        "repeated consent must not save again"
    );
    let listed = report(request(document, 1, Some(r#"{"type":"history.list"}"#)));
    assert!(
        listed["revisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|revision| revision["id"] == base.to_string())
    );
    unsafe { echode_document_close(document) };

    let document = unsafe { echode_document_open(c_path.as_ptr()) };
    assert!(report(request(document, ECHODE_DOCUMENT_SNAPSHOT, None))["recovery"].is_null());
    report(request(
        document,
        0,
        Some(r#"{"type":"track.add","name":"confirmed"}"#),
    ));
    unsafe { echode_document_close(document) };
    let recovered = echode_history::recover(&path).unwrap();
    assert_eq!(
        recovered
            .tracks()
            .iter()
            .map(|track| track.name.as_str())
            .collect::<Vec<_>>(),
        ["A", "confirmed"]
    );
    std::fs::remove_dir_all(path).unwrap();
}

fn gix_tip(path: &std::path::Path) -> String {
    std::fs::read_to_string(path.join(".git").join(AUTOSAVE_REF))
        .unwrap()
        .trim()
        .to_string()
}
