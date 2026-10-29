//! Exercise MCP framing through the public adapter and real private sockets.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use echode_api::{ApiSession, bind_socket, serve_connection, serve_mcp};
use echode_history::write_package;
use echode_model::{Id, Project};
use serde_json::{Value, json};

struct Fixture {
    directory: PathBuf,
    socket: PathBuf,
    session: Option<Arc<ApiSession>>,
    stop: Arc<AtomicBool>,
    connections: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        let id = Id::generate();
        let directory = std::env::temp_dir().join(format!("echode-mcp-framing-{id}"));
        let socket = PathBuf::from(format!("/tmp/echode-mcp-framing-{id}.sock"));
        write_package(&directory, &Project::new()).unwrap();
        let session = Arc::new(ApiSession::open(&directory).unwrap());
        let listener = bind_socket(&socket).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(AtomicUsize::new(0));
        let worker = {
            let session = Arc::clone(&session);
            let stop = Arc::clone(&stop);
            let connections = Arc::clone(&connections);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let stream = stream.unwrap();
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    connections.fetch_add(1, Ordering::Relaxed);
                    serve_connection(&session, stream).unwrap();
                }
            })
        };
        Self {
            directory,
            socket,
            session: Some(session),
            stop,
            connections,
            worker: Some(worker),
        }
    }

    fn call(&self, method: &str, params: Value) -> Value {
        let request = json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params});
        let response: Value = serde_json::from_str(
            &self
                .session
                .as_ref()
                .unwrap()
                .reply(&request.to_string())
                .unwrap(),
        )
        .unwrap();
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn summary(&self) -> Value {
        self.call("project.summary", json!({}))
    }

    fn close(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.stop.store(true, Ordering::Release);
            let _ = UnixStream::connect(&self.socket);
            worker.join().unwrap();
            self.session.take();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.close();
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn tool(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":7, "method":"tools/call", "params":{
        "name":name, "arguments":arguments, "_meta":{
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientCapabilities":{}
        }
    }})
}

fn responses(output: &[u8]) -> Vec<Value> {
    std::str::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn contract_unterminated_mcp_input_preserves_edits_history_and_recovery() {
    let mut fixture = Fixture::new();
    for name in ["kept", "redo"] {
        fixture.call("actions.perform", json!({"type":"track.add", "name":name}));
    }
    fixture.call("edit.undo", json!({}));
    let before = fixture.summary();
    let journal = fs::read(fixture.directory.join("journal.jsonl")).unwrap();
    for operation in [
        tool("echode.track.add", json!({"name":"unframed"})),
        tool(
            "echode.actions.transaction",
            json!({"actions":[{"type":"track.add", "name":"unframed"}]}),
        ),
        tool("echode.edit.undo", json!({})),
        tool("echode.edit.redo", json!({})),
    ] {
        for legacy in [false, true] {
            let mut operation = operation.clone();
            let prefix = if legacy {
                operation["params"].as_object_mut().unwrap().remove("_meta");
                concat!(
                    "{\"jsonrpc\":\"2.0\",\"id\":\"init\",\"method\":\"initialize\",\"params\":",
                    "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},",
                    "\"clientInfo\":{\"name\":\"regression\",\"version\":\"1\"}}}\n",
                    "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n"
                )
            } else {
                ""
            };
            // CR alone is JSON whitespace, not the newline frame delimiter.
            let input = format!("{prefix}{operation}\r");
            let mut output = Vec::new();
            let result = serve_mcp(&fixture.socket, input.as_bytes(), &mut output);
            assert_eq!(
                fs::read(fixture.directory.join("journal.jsonl")).unwrap(),
                journal,
                "an unterminated MCP frame must not execute an edit: {operation}"
            );
            assert_eq!(fixture.summary(), before);
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("without a newline")
            );
            assert_eq!(fixture.connections.load(Ordering::Relaxed), 0);
            let replies = responses(&output);
            assert_eq!(replies.len(), usize::from(legacy));
            if legacy {
                assert_eq!(replies[0]["id"], "init");
                assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
            }
        }
    }
    fixture.call("edit.redo", json!({}));
    assert_eq!(fixture.summary()["tracks"][1]["name"], "redo");
    fixture.call("edit.undo", json!({}));
    assert_eq!(fixture.summary(), before);

    // A complete prefix is committed; its following incomplete Undo is not run.
    let complete = tool("echode.track.add", json!({"name":"complete"}));
    let incomplete = tool("echode.edit.undo", json!({}));
    let input = format!("{complete}\r\n{incomplete}");
    let mut output = Vec::new();
    // Small read chunks also split JSON tokens and the CRLF delimiter.
    let result = serve_mcp(
        &fixture.socket,
        BufReader::with_capacity(1, input.as_bytes()),
        &mut output,
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("without a newline")
    );
    let replies = responses(&output);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["result"]["isError"], false);
    assert_eq!(fixture.connections.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.summary()["tracks"][1]["name"], "complete");
    fixture.call("edit.undo", json!({}));
    assert_eq!(fixture.summary(), before);
    fixture.call("edit.undo", json!({}));
    assert_eq!(fixture.summary()["tracks"], json!([]));
    fixture.call("edit.redo", json!({}));
    assert_eq!(fixture.summary(), before);
    fixture.call("edit.redo", json!({}));
    let expected = fixture.summary();
    assert_eq!(expected["tracks"][1]["name"], "complete");
    fixture.close();
    let reopened = ApiSession::open(&fixture.directory).unwrap();
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"project.summary"});
    let response: Value =
        serde_json::from_str(&reopened.reply(&request.to_string()).unwrap()).unwrap();
    assert_eq!(response["result"], expected);
}

#[test]
fn contract_unterminated_backend_frames_do_not_authorize_or_acknowledge_edits() {
    for handshake in [true, false] {
        for suffix in ["", "\r", " "] {
            let socket =
                PathBuf::from(format!("/tmp/echode-mcp-handshake-{}.sock", Id::generate()));
            let listener = bind_socket(&socket).unwrap();
            let worker = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                // These deadlines only bound this socket regression, not product work.
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut stream = BufReader::new(stream);
                let mut request = String::new();
                stream.read_line(&mut request).unwrap();
                let version: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(version["method"], "protocol.version");
                let mut response = json!({"jsonrpc":"2.0", "id":0, "result":{"version":1}});
                if !handshake {
                    writeln!(stream.get_mut(), "{response}").unwrap();
                    request.clear();
                    stream.read_line(&mut request).unwrap();
                    let edit: Value = serde_json::from_str(&request).unwrap();
                    assert_eq!(edit["method"], "actions.perform");
                    assert_eq!(edit["params"]["name"], "must not replay");
                    response = json!({"jsonrpc":"2.0", "id":1, "result":{}});
                }
                write!(stream.get_mut(), "{response}{suffix}").unwrap();
                stream.get_mut().shutdown(Shutdown::Write).unwrap();
                let mut following = String::new();
                stream.read_to_string(&mut following).unwrap_or_else(|error| {
                    panic!(
                        "backend EOF wait (handshake={handshake}, suffix={suffix:?}) after partial reply: {error}; following={following:?}"
                    )
                });
                following
            });
            let input = format!(
                "{}\n",
                tool("echode.track.add", json!({"name":"must not replay"}))
            );
            let mut output = Vec::new();
            let result = serve_mcp(&socket, input.as_bytes(), &mut output);
            let following = worker.join().unwrap();
            fs::remove_file(&socket).unwrap();
            result.unwrap();
            assert!(
                following.is_empty(),
                "a truncated backend frame must not authorize or replay an edit: {following}"
            );
            let replies = responses(&output);
            assert_eq!(replies.len(), 1);
            assert_eq!(replies[0]["id"], 7);
            assert_eq!(
                replies[0]["result"]["isError"], true,
                "a truncated backend frame must not acknowledge an edit"
            );
            let message = replies[0]["result"]["structuredContent"]["error"]["message"]
                .as_str()
                .unwrap();
            assert!(message.contains("without a newline"), "{message}");
            assert!(
                message.contains("Inspect the project before retrying"),
                "{message}"
            );
        }
    }
}

#[test]
fn stdio_message_size_counts_the_delimiter_and_eof_is_not_an_empty_frame() {
    // This is the existing per-message transport bound, not a project/track limit.
    const MESSAGE_LIMIT: usize = 1024 * 1024;
    let ping = json!({"jsonrpc":"2.0", "id":1, "method":"ping"}).to_string();
    let socket = std::path::Path::new("unused.sock");
    for delimiter in ["\n", "\r\n"] {
        let input = format!(
            "{ping}{}{delimiter}",
            " ".repeat(MESSAGE_LIMIT - ping.len() - delimiter.len())
        );
        let mut output = Vec::new();
        serve_mcp(socket, input.as_bytes(), &mut output).unwrap();
        assert_eq!(responses(&output)[0]["result"], json!({}));
        let oversized = format!(" {input}");
        output.clear();
        let error = serve_mcp(socket, oversized.as_bytes(), &mut output).unwrap_err();
        assert!(error.to_string().contains("size limit"));
        assert!(output.is_empty());
    }
    let mut output = Vec::new();
    serve_mcp(socket, &b""[..], &mut output).unwrap();
    assert!(output.is_empty());
    for input in ["{", " ", "\r", &ping] {
        let error = serve_mcp(socket, input.as_bytes(), &mut output).unwrap_err();
        assert!(error.to_string().contains("without a newline"));
        assert!(output.is_empty());
    }
}
