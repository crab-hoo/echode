//! A user-only Unix socket. One connection is served at a time.

use std::fs::{Permissions, set_permissions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use echode_model::Project;

use crate::rpc::ApiSession;

/// Why the socket stopped.
#[derive(Debug)]
pub struct ApiError {
    message: String,
}

impl ApiError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// Binds `path` and leaves it readable and writable only by the current user.
pub fn bind_socket(path: &Path) -> Result<UnixListener, ApiError> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| ApiError::new(error.to_string()))?;
    }
    let listener = UnixListener::bind(path).map_err(|error| ApiError::new(error.to_string()))?;
    set_permissions(path, Permissions::from_mode(0o600))
        .map_err(|error| ApiError::new(error.to_string()))?;
    Ok(listener)
}

/// Reads request lines until the client closes the connection.
pub fn serve_connection(session: &mut ApiSession, stream: UnixStream) -> Result<(), ApiError> {
    let mut writer = stream
        .try_clone()
        .map_err(|error| ApiError::new(error.to_string()))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| ApiError::new(error.to_string()))?;
        if read == 0 {
            return Ok(());
        }
        if let Some(response) = session.reply(&line) {
            writeln!(writer, "{response}").map_err(|error| ApiError::new(error.to_string()))?;
        }
    }
}

/// Accepts connections until the process stops. A new client sees earlier edits.
pub fn serve(project: Project, socket: &Path) -> Result<(), ApiError> {
    let listener = bind_socket(socket)?;
    let mut session = ApiSession::open(project);
    for incoming in listener.incoming() {
        let stream = incoming.map_err(|error| ApiError::new(error.to_string()))?;
        serve_connection(&mut session, stream)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    fn socket_path() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        std::path::PathBuf::from(format!("/tmp/echode-api-{}-{n}.sock", std::process::id()))
    }

    #[test]
    fn the_socket_is_private_and_lists_actions() {
        let path = socket_path();
        let listener = bind_socket(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let server = thread::spawn(move || {
            let stream = listener.accept().unwrap().0;
            let mut session = ApiSession::open(Project::new());
            serve_connection(&mut session, stream).unwrap();
        });
        let mut client = UnixStream::connect(&path).unwrap();
        writeln!(
            client,
            r#"{{"jsonrpc":"2.0","method":"actions.list","id":1}}"#
        )
        .unwrap();
        let mut reader = BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("track.add"));
        drop(reader);
        drop(client);
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }
}
