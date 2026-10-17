//! Exercise the real CLI over stdio and its existing private session socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use echode_history::{DocumentHost, write_package};
use echode_model::Project;
use serde_json::{Value, json};

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Fixture {
    directory: PathBuf,
    socket: PathBuf,
    server: Option<Process>,
    mcp: Option<Process>,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    next: u64,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let key = format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let directory = std::env::temp_dir().join(format!("echode-mcp-{key}"));
        let socket = PathBuf::from(format!("/tmp/echode-mcp-{key}.sock"));
        write_package(&directory, &Project::new()).unwrap();
        let server = Process(
            Command::new(env!("CARGO_BIN_EXE_echode"))
                .arg("serve")
                .arg(&directory)
                .arg(&socket)
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(directory.join("session-stderr.log")).unwrap())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&socket).is_err() {
            assert!(
                Instant::now() < deadline,
                "session socket did not become ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut mcp = Process(
            Command::new(env!("CARGO_BIN_EXE_echode"))
                .arg("mcp")
                .arg(&socket)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(std::fs::File::create(directory.join("mcp-stderr.log")).unwrap())
                .spawn()
                .unwrap(),
        );
        let input = mcp.0.stdin.take();
        let stdout = mcp.0.stdout.take().unwrap();
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value = line
                    .and_then(|line| serde_json::from_str(&line).map_err(std::io::Error::other));
                let Ok(value) = value else { break };
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            directory,
            socket,
            server: Some(server),
            mcp: Some(mcp),
            input,
            output,
            next: 0,
        }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.input.as_mut().unwrap(), "{message}").unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
    }

    fn request(&mut self, message: Value) -> Value {
        self.send(message);
        self.receive()
    }

    fn receive(&self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|error| {
                let stderr = std::fs::read_to_string(self.directory.join("mcp-stderr.log"))
                    .unwrap_or_default();
                panic!("MCP response: {error}; stderr: {stderr}");
            })
    }

    fn call(&mut self, method: &str, mut params: Value) -> Value {
        self.next += 1;
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        self.send(json!({"jsonrpc":"2.0", "id":self.next, "method":method, "params":params}));
        let value = self.receive();
        assert_eq!(
            value["id"], self.next,
            "notification or stray stdout: {value}"
        );
        value
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.call("tools/call", json!({"name":name,"arguments":arguments}))
    }

    fn close_mcp(&mut self) {
        self.input.take();
        let process = &mut self.mcp.as_mut().unwrap().0;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = process.try_wait().unwrap() {
                assert!(status.success(), "MCP did not exit cleanly: {status}");
                break;
            }
            assert!(Instant::now() < deadline, "MCP did not exit on EOF");
            std::thread::sleep(Duration::from_millis(10));
        }
        self.mcp.take();
    }
}

#[test]
fn legacy_clients_initialize_before_editing_and_cannot_replay_an_id() {
    let mut fixture = Fixture::new();
    let add = json!({"name":"echode.track.add","arguments":{"name":"Legacy track"}});
    let early =
        fixture.request(json!({"jsonrpc":"2.0","id":"early","method":"tools/call","params":add}));
    assert_eq!(early["error"]["code"], -32602);
    let initialized = fixture.request(json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"regression","version":"1"}
    }}));
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    let pending =
        fixture.request(json!({"jsonrpc":"2.0","id":"pending","method":"tools/call","params":add}));
    assert_eq!(pending["error"]["code"], -32602);
    fixture.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    fixture.send(json!({"jsonrpc":"2.0","method":"tools/call","params":add}));
    let request = json!({"jsonrpc":"2.0","id":"edit","method":"tools/call","params":add});
    let edited = fixture.request(request.clone());
    assert_eq!(edited["result"]["isError"], false);
    assert!(edited["result"].get("resultType").is_none());
    let repeated = fixture.request(request);
    assert_eq!(repeated["error"]["code"], -32600);
    let summary = fixture.tool("echode.project.summary", json!({}));
    assert_eq!(
        summary["result"]["structuredContent"]["result"]["tracks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fixture.close_mcp();
}

#[test]
fn malformed_calls_and_notifications_cannot_change_the_project() {
    let mut fixture = Fixture::new();
    let metadata = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});
    for id in [Value::Null, json!(true), json!([]), json!(1.5)] {
        let rejected = fixture.request(
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{
                "name":"echode.track.add","arguments":{"name":"must not appear"},"_meta":metadata
            }}),
        );
        assert_eq!(rejected["error"]["code"], -32600);
        assert!(rejected["id"].is_null());
    }
    fixture.send(json!({"jsonrpc":"2.0","method":"tools/call","params":{
        "name":"echode.track.add","arguments":{"name":"notification must not edit"},"_meta":metadata
    }}));
    for (method, params, code) in [
        (
            "tools/call",
            json!({"name":"unknown","arguments":{}}),
            -32602,
        ),
        (
            "tools/call",
            json!({"name":"echode.track.add","arguments":[]}),
            -32602,
        ),
        ("tools/list", json!({"cursor":"invented"}), -32602),
        ("unknown", json!({}), -32601),
        (
            "resources/read",
            json!({"uri":"file:///private/data"}),
            -32602,
        ),
    ] {
        assert_eq!(fixture.call(method, params)["error"]["code"], code);
    }
    for (name, arguments) in [
        ("echode.track.add", json!({"name":7})),
        ("echode.track.add", json!({"name":"bad","type":"track.add"})),
        ("echode.edit.undo", json!({"actions":[]})),
        (
            "echode.actions.transaction",
            json!({"actions":[],"extra":true}),
        ),
    ] {
        let rejected = fixture.tool(name, arguments);
        assert_eq!(rejected["result"]["isError"], true, "{rejected}");
    }
    let summary = fixture.tool("echode.project.summary", json!({}));
    assert_eq!(
        summary["result"]["structuredContent"]["result"]["tracks"],
        json!([])
    );
    let resources = fixture.call("resources/list", json!({}));
    assert_eq!(resources["result"]["cacheScope"], "private");
    assert_eq!(resources["result"]["ttlMs"], 0);
    let tracks = fixture.call("resources/read", json!({"uri":"echode://project/tracks"}));
    assert_eq!(tracks["result"]["contents"][0]["text"], "[]");
    fixture.close_mcp();
}

#[test]
fn modern_requests_validate_their_own_metadata_and_report_a_stopped_session() {
    let mut fixture = Fixture::new();
    for (meta, code) in [
        (
            json!({"io.modelcontextprotocol/protocolVersion":"2099-01-01","io.modelcontextprotocol/clientCapabilities":{}}),
            -32022,
        ),
        (
            json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28"}),
            -32602,
        ),
        (
            json!({"io.modelcontextprotocol/protocolVersion":7,"io.modelcontextprotocol/clientCapabilities":{}}),
            -32602,
        ),
    ] {
        let response = fixture.request(
            json!({"jsonrpc":"2.0","id":"invalid","method":"tools/list","params":{"_meta":meta}}),
        );
        assert_eq!(response["error"]["code"], code);
    }
    let good = fixture.call("tools/list", json!({}));
    assert_eq!(good["result"]["resultType"], "complete");
    let missing =
        fixture.request(json!({"jsonrpc":"2.0","id":"missing-meta","method":"tools/list"}));
    assert_eq!(missing["error"]["code"], -32602);
    fixture.server.take();
    let failed = fixture.tool("echode.track.add", json!({"name":"unavailable"}));
    assert_eq!(failed["result"]["isError"], true);
    assert!(
        failed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("before retrying")
    );
    fixture.close_mcp();
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.input.take();
        self.mcp.take();
        self.server.take();
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn contract_mcp_stdio_edits_share_one_durable_session() {
    let mut fixture = Fixture::new();
    let discover = fixture.call("server/discover", json!({}));
    assert_eq!(discover["result"]["supportedVersions"][0], "2026-07-28");
    let tools = fixture.call("tools/list", json!({}));
    let tools = tools["result"]["tools"].as_array().unwrap();
    let add = tools
        .iter()
        .find(|tool| tool["name"] == "echode.track.add")
        .unwrap();
    assert_eq!(add["inputSchema"]["properties"]["name"]["type"], "string");
    assert!(add["inputSchema"]["properties"].get("type").is_none());
    assert_eq!(add["annotations"]["readOnlyHint"], false);

    let edited = fixture.tool(
        "echode.actions.transaction",
        json!({"actions":[
            {"type":"track.add","name":"MCP piano"},
            {"type":"note.insert","track":{"ref":0,"field":"id"},
             "pitch":60,"velocity":100,"start":0,"length":705600}
        ]}),
    );
    assert_eq!(edited["result"]["isError"], false, "{edited}");
    let track = edited["result"]["structuredContent"]["result"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let summary = fixture.call("resources/read", json!({"uri":"echode://project/summary"}));
    let summary: Value =
        serde_json::from_str(summary["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(summary["tracks"][0]["id"], track);
    assert_eq!(summary["tracks"][0]["clips"], 1);

    assert_eq!(
        fixture.tool("echode.edit.undo", json!({}))["result"]["isError"],
        false
    );
    let empty = fixture.tool("echode.project.summary", json!({}));
    assert_eq!(
        empty["result"]["structuredContent"]["result"]["tracks"],
        json!([])
    );
    assert_eq!(
        fixture.tool("echode.edit.redo", json!({}))["result"]["isError"],
        false
    );
    let rejected = fixture.tool(
        "echode.actions.transaction",
        json!({"actions":[
            {"type":"track.add","name":"must roll back"},
            {"type":"project.set_meter","numerator":0,"denominator":4}
        ]}),
    );
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["structuredContent"]["error"]["data"]["index"],
        1
    );
    let added = fixture.tool("echode.track.add", json!({"name":"Second track"}));
    assert_eq!(added["result"]["isError"], false, "{added}");
    // An idle stdio client must not occupy the session's single connection.
    let mut direct = UnixStream::connect(&fixture.socket).unwrap();
    direct
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(
        direct,
        "{}",
        json!({"jsonrpc":"2.0","id":99,"method":"project.summary"})
    )
    .unwrap();
    let mut reply = String::new();
    BufReader::new(&mut direct).read_line(&mut reply).unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["result"]["tracks"][0]["id"], track);
    drop(direct);
    fixture.close_mcp();
    fixture.server.take();
    let host = DocumentHost::open(&fixture.directory).unwrap();
    let project = host.snapshot();
    assert_eq!(project.tracks().len(), 2);
    assert_eq!(project.tracks()[0].id.to_string(), track);
    assert_eq!(project.tracks()[0].name, "MCP piano");
    assert_eq!(project.tracks()[1].name, "Second track");
    project.check_invariants().unwrap();
    drop(host);
}
