//! Open items from docs/reviews/2026-10-08-audit.md. Each test fails until its item
//! is fixed; `make known-bugs` runs them. Remove the `#[ignore]` with the fix.

use echode_api::bind_socket;

#[test]
fn audit_api3_binding_over_a_regular_file_keeps_the_file() {
    let path = std::env::temp_dir().join(format!("echode-audit-api3-{}.json", std::process::id()));
    std::fs::write(&path, b"{}").unwrap();
    let bound = bind_socket(&path);
    let kept = std::fs::read(&path).ok();
    let _ = std::fs::remove_file(&path);
    assert!(bound.is_err(), "a regular file was replaced by a socket");
    assert_eq!(kept.as_deref(), Some(&b"{}"[..]));
}

/// DIA-01 (docs/reviews/2026-11-13-detailed-implementation-audit.md): the MCP
/// adapter reads the session's reply under the request-line limit. A committed
/// transaction answers with every created id, so its reply is longer than its
/// request, and one past the limit is reported as a failed session request.
#[test]
#[ignore = "DIA-01: a committed edit whose reply passes 1 MiB is reported as failed"]
fn audit_dia01_a_committed_transaction_with_a_long_reply_is_reported_as_done() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    use echode_api::{ApiSession, serve_connection, serve_mcp};
    use echode_model::{Id, Project};
    use serde_json::{Value, json};

    const LIMIT: usize = 1024 * 1024;
    let id = Id::generate();
    let directory = std::env::temp_dir().join(format!("echode-audit-dia01-{id}"));
    let socket = std::path::PathBuf::from(format!("/tmp/echode-audit-dia01-{id}.sock"));
    echode_history::write_package(&directory, &Project::new()).unwrap();
    let session = Arc::new(ApiSession::open(&directory).unwrap());
    // Few actions, so the edit is quick; long names bring the request near the
    // limit, and the six ids each reply adds take the reply past it.
    let actions: Vec<Value> = (0..200)
        .map(|_| json!({"type":"track.add", "name":"n".repeat(5_060)}))
        .collect();
    let request = json!({"jsonrpc":"2.0", "id":7, "method":"tools/call", "params":{
        "name":"echode.actions.transaction", "arguments":{"actions":actions}, "_meta":{
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientCapabilities":{}
        }
    }});
    let sent = request.to_string().len();
    assert!(
        sent + 20_000 < LIMIT,
        "the request is within the limit: {sent}"
    );

    let listener = bind_socket(&socket).unwrap();
    let backend = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let _ = serve_connection(&session, stream);
        })
    };
    let (client, server) = UnixStream::pair().unwrap();
    let adapter = {
        let socket = socket.clone();
        std::thread::spawn(move || {
            serve_mcp(&socket, BufReader::new(server.try_clone().unwrap()), server)
        })
    };
    let mut client = BufReader::new(client);
    writeln!(client.get_mut(), "{request}").unwrap();
    let mut line = String::new();
    client.read_line(&mut line).unwrap();
    client
        .get_mut()
        .shutdown(std::net::Shutdown::Write)
        .unwrap();
    adapter.join().unwrap().unwrap();
    backend.join().unwrap();

    let usage: Value = serde_json::from_str(
        &session
            .reply(&json!({"jsonrpc":"2.0", "id":2, "method":"history.usage"}).to_string())
            .unwrap(),
    )
    .unwrap();
    drop(session);
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        usage["result"]["undo_steps"], 1,
        "the transaction was committed"
    );
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        answer["result"]["isError"], false,
        "a committed edit was reported as failed: {}",
        answer["result"]["structuredContent"]["error"]
    );
}
