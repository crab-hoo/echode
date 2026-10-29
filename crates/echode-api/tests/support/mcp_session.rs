//! A real document backend shared by public MCP input regressions.

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;

use echode_api::{ApiSession, bind_socket, serve_connection};
use echode_history::write_package;
use echode_model::{Id, Project};
use serde_json::{Value, json};

pub struct Fixture {
    pub directory: PathBuf,
    pub socket: PathBuf,
    session: Option<Arc<ApiSession>>,
    stop: Arc<AtomicBool>,
    pub connections: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
}

impl Fixture {
    pub fn new() -> Self {
        let id = Id::generate();
        let directory = std::env::temp_dir().join(format!("echode-mcp-input-{id}"));
        let socket = PathBuf::from(format!("/tmp/echode-mcp-input-{id}.sock"));
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

    pub fn call(&self, method: &str, params: Value) -> Value {
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

    pub fn summary(&self) -> Value {
        self.call("project.summary", json!({}))
    }

    pub fn close(&mut self) {
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

pub fn tool(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":7, "method":"tools/call", "params":{
        "name":name, "arguments":arguments, "_meta":{
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientCapabilities":{}
        }
    }})
}

pub fn responses(output: &[u8]) -> Vec<Value> {
    std::str::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
