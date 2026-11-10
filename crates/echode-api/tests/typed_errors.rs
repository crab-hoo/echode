//! Public transport boundaries must keep their original failures until display.

use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;

use echode_api::{ApiError, ApiIoOperation, bind_socket, serve, serve_mcp};
use echode_history::PackageError;
use echode_model::Id;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let directory = PathBuf::from(format!("/tmp/echode-api-errors-{}", Id::generate()));
        std::fs::create_dir(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn typed_errors_socket_bind_keeps_the_io_cause_and_errno() {
    let fixture = Fixture::new();
    let error = bind_socket(&fixture.0.join("missing/session.sock")).unwrap_err();
    assert!(matches!(
        &error,
        ApiError::Io {
            operation: ApiIoOperation::BindSocket,
            ..
        }
    ));
    let source = error
        .source()
        .and_then(|source| source.downcast_ref::<io::Error>())
        .expect("socket binding must retain the original I/O cause");
    assert_eq!(source.raw_os_error(), Some(libc::ENOENT));
    assert_eq!(source.kind(), io::ErrorKind::NotFound);
    assert_eq!(error.to_string(), source.to_string());
}

#[test]
fn typed_errors_serve_keeps_the_package_cause() {
    let fixture = Fixture::new();
    let error = serve(&fixture.0.join("missing"), &fixture.0.join("session.sock")).unwrap_err();
    assert!(matches!(&error, ApiError::Package(_)));
    let source = error
        .source()
        .and_then(|source| source.downcast_ref::<PackageError>())
        .expect("serving must retain the original package cause");
    assert!(matches!(
        source,
        PackageError::Io { .. } | PackageError::MissingDirectory(_)
    ));
    assert_eq!(error.to_string(), source.to_string());
}

struct FailedOutput {
    flush: bool,
}

impl Write for FailedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.flush {
            Ok(bytes.len())
        } else {
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::EPIPE))
    }
}

#[test]
fn typed_errors_mcp_stdio_keeps_read_write_and_flush_causes() {
    let fixture = Fixture::new();
    let socket = fixture.0.join("unused.sock");
    let read = serve_mcp(&socket, &b"\xff\n"[..], Vec::new()).unwrap_err();
    assert!(matches!(
        &read,
        ApiError::Io {
            operation: ApiIoOperation::ReadMcpMessage,
            ..
        }
    ));
    let source = read
        .source()
        .and_then(|source| source.downcast_ref::<io::Error>())
        .expect("MCP reading must retain the original I/O cause");
    assert_eq!(source.kind(), io::ErrorKind::InvalidData);
    assert_eq!(read.to_string(), source.to_string());

    for (flush, errno) in [(false, libc::ENOSPC), (true, libc::EPIPE)] {
        let error = serve_mcp(
            &socket,
            &b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n"[..],
            FailedOutput { flush },
        )
        .unwrap_err();
        let expected = if flush {
            ApiIoOperation::FlushMcpReply
        } else {
            ApiIoOperation::WriteMcpReply
        };
        assert!(matches!(
            &error,
            ApiError::Io { operation, .. } if *operation == expected
        ));
        let source = error
            .source()
            .and_then(|source| source.downcast_ref::<io::Error>())
            .expect("MCP output must retain the original I/O cause");
        assert_eq!(source.raw_os_error(), Some(errno));
        assert_eq!(error.to_string(), source.to_string());
    }
}

#[test]
fn typed_errors_socket_path_rejections_keep_the_original_entries() {
    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    std::fs::write(&target, b"keep").unwrap();
    let link = fixture.0.join("link.sock");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let error = bind_socket(&link).unwrap_err();
    assert!(matches!(error, ApiError::SocketPathSymlink));
    assert_eq!(error.to_string(), "socket path is a symbolic link");
    assert!(error.source().is_none());
    assert_eq!(std::fs::read_link(link).unwrap(), target);
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");

    let error = bind_socket(&fixture.0).unwrap_err();
    assert!(matches!(error, ApiError::SocketPathNotSocket));
    assert_eq!(error.to_string(), "socket path is not a socket");
    assert!(error.source().is_none());
    assert!(fixture.0.is_dir());
}
