//! A user-only Unix socket. One connection is served at a time.
//!
//! A regular file at the socket path is left untouched. A live socket is left
//! to its owner. Only a socket whose listener has gone away is replaced.

use std::fs::{Permissions, set_permissions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use crate::rpc::ApiSession;

/// One request line. This bounds a single JSON-RPC message, not the project.
pub(crate) const MAX_REQUEST_LINE: usize = 1024 * 1024;

/// How long one connection may sit without sending a byte. A silent client
/// must not hold the accept loop forever.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Why the socket stopped.
#[derive(Debug)]
pub struct ApiError {
    message: String,
}

impl ApiError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
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
    prepare_socket_path(path)?;
    let listener = UnixListener::bind(path).map_err(|error| ApiError::new(error.to_string()))?;
    set_permissions(path, Permissions::from_mode(0o600))
        .map_err(|error| ApiError::new(error.to_string()))?;
    Ok(listener)
}

/// Reads request lines until the client closes the connection or sits idle.
pub fn serve_connection(session: &ApiSession, stream: UnixStream) -> Result<(), ApiError> {
    serve_connection_limited(session, stream, READ_TIMEOUT, MAX_REQUEST_LINE)
}

fn serve_connection_limited(
    session: &ApiSession,
    stream: UnixStream,
    timeout: Duration,
    max_line: usize,
) -> Result<(), ApiError> {
    if peer_uid(&stream)? != current_uid() {
        return Err(ApiError::new("connection is from another user"));
    }
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| ApiError::new(error.to_string()))?;
    let mut writer = stream
        .try_clone()
        .map_err(|error| ApiError::new(error.to_string()))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let limit = u64::try_from(max_line)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let read = (&mut reader).take(limit).read_line(&mut line);
        match read {
            Ok(0) => return Ok(()),
            Ok(_) if line.len() > max_line => {
                return Err(ApiError::new("request line is too long"));
            }
            Ok(_) => {}
            Err(error)
                if error.kind() == ErrorKind::TimedOut || error.kind() == ErrorKind::WouldBlock =>
            {
                return Ok(());
            }
            Err(error) => return Err(ApiError::new(error.to_string())),
        }
        if let Some(response) = session.reply(&line) {
            writeln!(writer, "{response}").map_err(|error| ApiError::new(error.to_string()))?;
        }
    }
}

fn prepare_socket_path(path: &Path) -> Result<(), ApiError> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(ApiError::new(error.to_string())),
    };
    let kind = metadata.file_type();
    if kind.is_symlink() {
        return Err(ApiError::new("socket path is a symbolic link"));
    }
    if kind.is_file() {
        return Err(ApiError::new("socket path is a regular file"));
    }
    if !kind.is_socket() {
        return Err(ApiError::new("socket path is not a socket"));
    }
    match UnixStream::connect(path) {
        Ok(stream) => {
            drop(stream);
            Err(ApiError::new("socket is already in use"))
        }
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
            std::fs::remove_file(path).map_err(|error| ApiError::new(error.to_string()))
        }
        Err(error) => Err(ApiError::new(error.to_string())),
    }
}

pub(crate) fn peer_uid(stream: &UnixStream) -> Result<u32, ApiError> {
    // libc declares getpeereid. This process never calls it from the audio thread.
    let mut uid = 0;
    let mut gid = 0;
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err(ApiError::new(std::io::Error::last_os_error().to_string()));
    }
    Ok(uid)
}

pub(crate) fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

/// Accepts connections until the process stops. A new client sees earlier edits.
pub fn serve(package: &Path, socket: &Path) -> Result<(), ApiError> {
    let listener = bind_socket(socket)?;
    let session = ApiSession::open(package).map_err(|error| ApiError::new(error.to_string()))?;
    for incoming in listener.incoming() {
        let stream = incoming.map_err(|error| ApiError::new(error.to_string()))?;
        // One client ending early, sitting idle, or sending a bad line does
        // not close the socket for the next client.
        let _ = serve_connection(&session, stream);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_model::Project;
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
            let directory =
                std::env::temp_dir().join(format!("echode-api-pkg-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            echode_history::write_package(&directory, &Project::new()).unwrap();
            let session = ApiSession::open(&directory).unwrap();
            let result = serve_connection(&session, stream);
            drop(session);
            let _ = std::fs::remove_dir_all(directory);
            result.unwrap();
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

    #[test]
    fn a_regular_file_is_left_in_place() {
        let path = socket_path();
        std::fs::write(&path, b"{}").unwrap();
        let error = bind_socket(&path).unwrap_err();
        assert!(error.to_string().contains("regular file"));
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_live_socket_is_not_replaced() {
        let path = socket_path();
        let listener = bind_socket(&path).unwrap();
        let error = bind_socket(&path).unwrap_err();
        assert!(error.to_string().contains("already in use"));
        assert!(path.symlink_metadata().unwrap().file_type().is_socket());
        drop(listener);
        let replaced = bind_socket(&path).unwrap();
        drop(replaced);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_silent_client_releases_the_connection() {
        let path = socket_path();
        let listener = bind_socket(&path).unwrap();
        let (done, arrived) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let stream = listener.accept().unwrap().0;
            let directory =
                std::env::temp_dir().join(format!("echode-api-idle-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            echode_history::write_package(&directory, &Project::new()).unwrap();
            let session = ApiSession::open(&directory).unwrap();
            let result = serve_connection_limited(
                &session,
                stream,
                Duration::from_millis(200),
                MAX_REQUEST_LINE,
            );
            let _ = std::fs::remove_dir_all(directory);
            let _ = done.send(result.map(|_| ()));
        });
        let client = UnixStream::connect(&path).unwrap();
        let result = arrived.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(client);
        assert!(result.is_ok());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_request_line_past_the_cap_is_refused() {
        let path = socket_path();
        let listener = bind_socket(&path).unwrap();
        let (done, arrived) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let stream = listener.accept().unwrap().0;
            let directory =
                std::env::temp_dir().join(format!("echode-api-line-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            echode_history::write_package(&directory, &Project::new()).unwrap();
            let session = ApiSession::open(&directory).unwrap();
            let result = serve_connection_limited(&session, stream, READ_TIMEOUT, 32);
            let _ = std::fs::remove_dir_all(directory);
            let _ = done.send(result.map_err(|error| error.to_string()));
        });
        let mut client = UnixStream::connect(&path).unwrap();
        client.write_all(&[b'a'; 64]).unwrap();
        let error = arrived.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(client);
        assert!(error.unwrap_err().contains("too long"));
        let _ = std::fs::remove_file(path);
    }
}
