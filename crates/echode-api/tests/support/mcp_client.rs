//! An interactive public MCP stream without launching a CLI or opening a device.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use echode_api::{ApiError, serve_mcp};
use serde_json::{Value, json};

pub struct Client {
    stream: BufReader<UnixStream>,
    worker: Option<JoinHandle<Result<(), ApiError>>>,
    barrier: u64,
}

impl Client {
    pub fn new(socket: &Path) -> Self {
        let (client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let socket = socket.to_owned();
        let worker = std::thread::spawn(move || {
            serve_mcp(&socket, BufReader::new(server.try_clone().unwrap()), server)
        });
        Self {
            stream: BufReader::new(client),
            worker: Some(worker),
            barrier: 0,
        }
    }

    pub fn request(&mut self, request: Value) -> Value {
        writeln!(self.stream.get_mut(), "{request}").unwrap();
        let mut line = String::new();
        assert_ne!(self.stream.read_line(&mut line).unwrap(), 0);
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], request["id"]);
        reply
    }

    pub fn notify(&mut self, notification: Value) {
        assert!(notification.get("id").is_none());
        writeln!(self.stream.get_mut(), "{notification}").unwrap();
        // A later reply proves the notification was processed without producing one.
        self.barrier += 1;
        let reply = self.request(json!({
            "jsonrpc":"2.0", "id":format!("barrier-{}", self.barrier), "method":"ping"
        }));
        assert_eq!(reply["result"], json!({}));
    }

    pub fn close(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.stream.get_mut().shutdown(Shutdown::Write).unwrap();
            worker.join().unwrap().unwrap();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.stream.get_mut().shutdown(Shutdown::Write);
            let _ = worker.join();
        }
    }
}
