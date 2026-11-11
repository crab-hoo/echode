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

use crate::error::{ApiError, ApiIoOperation};
use crate::rpc::ApiSession;

/// One request line. This bounds a single JSON-RPC message, not the project.
pub(crate) const MAX_REQUEST_LINE: usize = 1024 * 1024;

/// How long a connection may wait for incoming bytes or writable reply space.
/// A silent client or unread reply must not hold the accept loop forever.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Binds `path` and leaves it readable and writable only by the current user.
pub fn bind_socket(path: &Path) -> Result<UnixListener, ApiError> {
    prepare_socket_path(path)?;
    let listener = UnixListener::bind(path)
        .map_err(|error| ApiError::io(ApiIoOperation::BindSocket, error))?;
    set_permissions(path, Permissions::from_mode(0o600))
        .map_err(|error| ApiError::io(ApiIoOperation::SetSocketPermissions, error))?;
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
        return Err(ApiError::ConnectionFromAnotherUser);
    }
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| ApiError::io(ApiIoOperation::SetReadTimeout, error))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| ApiError::io(ApiIoOperation::SetWriteTimeout, error))?;
    let mut writer = stream
        .try_clone()
        .map_err(|error| ApiError::io(ApiIoOperation::CloneStream, error))?;
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
                return Err(ApiError::RequestLineTooLong);
            }
            Ok(_) if !line.ends_with('\n') => {
                // EOF is not a frame delimiter, even for syntactically valid JSON.
                return Err(ApiError::RequestLineIncomplete);
            }
            Ok(_) => {}
            Err(error)
                if error.kind() == ErrorKind::TimedOut || error.kind() == ErrorKind::WouldBlock =>
            {
                return Ok(());
            }
            Err(error) => return Err(ApiError::io(ApiIoOperation::ReadRequest, error)),
        }
        if let Some(response) = session.reply(&line) {
            // An edit may already be committed. Close on write failure without
            // replaying the request, even if only part of the reply was sent.
            writeln!(writer, "{response}")
                .map_err(|error| ApiError::io(ApiIoOperation::WriteReply, error))?;
        }
    }
}

fn prepare_socket_path(path: &Path) -> Result<(), ApiError> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(ApiError::io(ApiIoOperation::InspectSocketPath, error)),
    };
    let kind = metadata.file_type();
    if kind.is_symlink() {
        return Err(ApiError::SocketPathSymlink);
    }
    if kind.is_file() {
        return Err(ApiError::SocketPathRegularFile);
    }
    if !kind.is_socket() {
        return Err(ApiError::SocketPathNotSocket);
    }
    match UnixStream::connect(path) {
        Ok(stream) => {
            drop(stream);
            Err(ApiError::SocketInUse)
        }
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => std::fs::remove_file(path)
            .map_err(|error| ApiError::io(ApiIoOperation::RemoveStaleSocket, error)),
        Err(error) => Err(ApiError::io(ApiIoOperation::ProbeSocket, error)),
    }
}

pub(crate) fn peer_uid(stream: &UnixStream) -> Result<u32, ApiError> {
    // libc declares getpeereid. This process never calls it from the audio thread.
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: `uid` and `gid` are writable locals and the descriptor is the connected socket's own.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err(ApiError::io(
            ApiIoOperation::PeerCredentials,
            std::io::Error::last_os_error(),
        ));
    }
    Ok(uid)
}

pub(crate) fn current_uid() -> u32 {
    // SAFETY: `getuid` has no preconditions.
    unsafe { libc::getuid() }
}

/// Accepts connections until the process stops. A new client sees earlier edits.
pub fn serve(package: &Path, socket: &Path) -> Result<(), ApiError> {
    let listener = bind_socket(socket)?;
    let session = ApiSession::open(package).map_err(ApiError::Package)?;
    for incoming in listener.incoming() {
        let stream = incoming.map_err(|error| ApiError::io(ApiIoOperation::Accept, error))?;
        // One client ending early, sitting idle, or sending a bad line does
        // not close the socket for the next client.
        let _ = serve_connection(&session, stream);
    }
    Ok(())
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
