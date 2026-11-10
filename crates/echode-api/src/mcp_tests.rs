use super::*;
use crate::bind_socket;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn stdio_keeps_parse_errors_out_of_the_following_reply_and_closes_at_eof() {
    let input = b"{bad}\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n";
    let mut output = Vec::new();
    serve_mcp(Path::new("unused.sock"), &input[..], &mut output).unwrap();
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["id"], 7);
    assert_eq!(lines[1]["result"], json!({}));
}

#[test]
fn a_message_limit_and_truncated_line_do_not_parse_another_request() {
    // This is the transport's per-message bound, not a project/track limit.
    let oversized = vec![b'x'; MAX_REQUEST_LINE + 1];
    let mut output = Vec::new();
    let error = serve_mcp(Path::new("unused.sock"), &oversized[..], &mut output).unwrap_err();
    assert!(matches!(error, ApiError::McpMessageTooLong));
    assert!(output.is_empty());
    let error = serve_mcp(Path::new("unused.sock"), &b"{}"[..], &mut output).unwrap_err();
    assert!(matches!(error, ApiError::McpMessageIncomplete));
    assert!(output.is_empty());
}

#[test]
fn a_wrong_session_version_never_receives_an_edit() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::path::PathBuf::from(format!(
        "/tmp/echode-mcp-version-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let listener = bind_socket(&path).unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = BufReader::new(listener.accept().unwrap().0);
        let request: Value =
            serde_json::from_str(&read_line(&mut stream).unwrap().unwrap()).unwrap();
        assert_eq!(request["method"], "protocol.version");
        writeln!(
            stream.get_mut(),
            "{}",
            json!({"jsonrpc":"2.0","id":0,"result":{"version":2}})
        )
        .unwrap();
        assert!(
            read_line(&mut stream).unwrap().is_none(),
            "an incompatible session received an edit"
        );
    });
    let result = backend(
        &path,
        "actions.perform",
        json!({"type":"track.add","name":"must not arrive"}),
    );
    server.join().unwrap();
    std::fs::remove_file(path).unwrap();
    let error = result.unwrap_err();
    assert!(matches!(error, ApiError::UnsupportedSessionProtocol));
    assert_eq!(error.to_string(), "unsupported Echode session protocol");
}

#[test]
fn malformed_or_mismatched_backend_results_are_not_successful_edits() {
    for (response, kind, message) in [
        ("not json\n".to_string(), "json", ""),
        (
            json!({"jsonrpc":"2.0","id":9,"result":{}}).to_string() + "\n",
            "envelope",
            "invalid session response",
        ),
        (
            json!({"jsonrpc":"2.0","id":1,"result":{},"error":{}}).to_string() + "\n",
            "envelope",
            "invalid session response",
        ),
        (
            json!({"jsonrpc":"1.0","id":1,"result":{}}).to_string() + "\n",
            "envelope",
            "invalid session response",
        ),
        (
            "{}".to_string(),
            "framing",
            "MCP message ended without a newline",
        ),
        (String::new(), "closed", "session closed without a response"),
    ] {
        let (client, server) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut server = BufReader::new(server);
            read_line(&mut server).unwrap().unwrap();
            server.get_mut().write_all(response.as_bytes()).unwrap();
        });
        let error =
            exchange(&mut BufReader::new(client), 1, "project.summary", json!({})).unwrap_err();
        server.join().unwrap();
        match kind {
            "json" => {
                let ApiError::SessionJson(json) = &error else {
                    panic!("{error:?}")
                };
                assert_eq!(json.classify(), serde_json::error::Category::Syntax);
                assert!(error.source().unwrap().is::<serde_json::Error>());
                assert_eq!(error.to_string(), json.to_string());
            }
            "envelope" => assert!(matches!(error, ApiError::InvalidSessionResponse)),
            "framing" => assert!(matches!(error, ApiError::McpMessageIncomplete)),
            "closed" => assert!(matches!(error, ApiError::SessionClosed)),
            _ => unreachable!(),
        }
        if !message.is_empty() {
            assert_eq!(error.to_string(), message);
            assert!(error.source().is_none());
        }
    }
}

#[test]
fn typed_errors_session_connect_is_kept_until_the_resource_reply() {
    let path = std::path::PathBuf::from(format!(
        "/tmp/echode-mcp-absent-{}.sock",
        echode_model::Id::generate()
    ));
    let mut session = McpSession {
        socket: &path,
        lifecycle: Lifecycle::Fresh,
        legacy_ids: BTreeSet::new(),
    };
    let problem = session
        .dispatch("resources/read", &json!({"uri": SUMMARY_URI}), true)
        .unwrap_err();
    let ProtocolMessage::Session(session_error) = &problem.message else {
        panic!("{problem:?}")
    };
    let ApiError::Io {
        operation,
        error: source,
    } = session_error.as_ref()
    else {
        panic!("{problem:?}")
    };
    assert_eq!(*operation, ApiIoOperation::ConnectSession);
    assert_eq!(source.raw_os_error(), Some(libc::ENOENT));
    let message = source.to_string();
    assert_eq!(
        error(&json!(1), problem),
        json!({"jsonrpc": "2.0", "id": 1, "error": {
            "code": -32603, "message": message
        }})
    );

    let tool = session
        .reply(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": "echode.track.add", "arguments": {"name": "not sent"},
                "_meta": {VERSION_KEY: MODERN, CAPABILITIES_KEY: {}}
            }})
            .to_string(),
        )
        .unwrap();
    assert_eq!(tool["result"]["isError"], true);
    assert_eq!(
        tool["result"]["structuredContent"]["error"]["message"],
        format!(
            "Session request failed: {message}. Inspect the project before retrying an edit; its outcome may be unknown."
        )
    );
}

#[test]
fn typed_errors_session_request_limit_does_not_send_bytes() {
    let (client, mut server) = UnixStream::pair().unwrap();
    // The fixture exceeds only the single-message limit, not any project limit.
    let arguments = json!({"name": "x".repeat(MAX_REQUEST_LINE)});
    let error = exchange(&mut BufReader::new(client), 1, "actions.perform", arguments).unwrap_err();
    assert!(matches!(error, ApiError::SessionRequestTooLong));
    let mut bytes = Vec::new();
    server.read_to_end(&mut bytes).unwrap();
    assert!(bytes.is_empty());
}
