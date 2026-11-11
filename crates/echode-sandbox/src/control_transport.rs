//! The control socket's transport: how a message is split into frames on
//! send, and the descriptor rules a peer can break on receive. Timeouts, end
//! of file and zero-byte writes are told apart by `ErrorKind`.
//!
//! A frame's length field (type, request id and body) is at most
//! [`MAX_FRAME`], so a corrupt peer cannot make the receiver hold more than
//! one frame for one length field. That bounds a frame, not a message: a body
//! past [`MAX_BODY`] goes as `PART` frames of the same request id, then one
//! frame of its own type with the rest (ADR 0318). The receiver joins them, so
//! a message holds only bytes the peer actually sent.

use std::io::{self, ErrorKind};

/// The largest length field a receiver accepts: 64 MiB.
pub(crate) const MAX_FRAME: usize = 64 * 1024 * 1024;
/// The largest body one frame carries: the length field also counts the type
/// and the request id.
pub(crate) const MAX_BODY: usize = MAX_FRAME - 12;
/// The wire type of a frame that carries the leading bytes of a longer body.
/// Its request id is the message's; the frame with the message's own type
/// follows with the rest of the body.
pub(crate) const PART: u32 = 35;

/// How a peer broke the rule of at most one descriptor per message, consumed
/// before the next arrives (SBX-A1 (2), ADR 0288). The read's bytes are
/// dropped, every descriptor it carried is closed, and each later receive on
/// the socket returns the same cause, so the message is never accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorRuleError {
    /// The kernel truncated the ancillary data. `received` is how many whole
    /// descriptors the read still delivered; all were closed.
    ControlTruncated { received: usize },
    /// One read carried `count` descriptors, more than one.
    ExtraDescriptors { count: usize },
    /// A descriptor arrived while an earlier one still waited for its message.
    UnconsumedDescriptor,
}

impl std::fmt::Display for DescriptorRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ControlTruncated { .. } => "control data was truncated",
            Self::ExtraDescriptors { .. } => {
                "a control message carried more than one file descriptor"
            }
            Self::UnconsumedDescriptor => {
                "a second file descriptor arrived before the first was consumed"
            }
        })
    }
}

impl std::error::Error for DescriptorRuleError {}

impl From<DescriptorRuleError> for std::io::Error {
    fn from(error: DescriptorRuleError) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, error)
    }
}

/// Writes one message as frames of at most `max_body` body bytes: `PART`
/// frames while more than `max_body` bytes remain, then a frame of `typ` with
/// the rest. A descriptor goes with the frame of `typ`. The product passes
/// [`MAX_BODY`]; tests pass less to split small bodies.
pub(crate) fn send_message(
    socket: i32,
    id: u64,
    typ: u32,
    body: &[u8],
    fd: Option<i32>,
    max_body: usize,
) -> io::Result<()> {
    let max_body = max_body.clamp(1, MAX_BODY);
    let mut rest = body;
    while let Some((part, tail)) = rest.split_at_checked(max_body)
        && !tail.is_empty()
    {
        write_frame(socket, id, PART, part, None)?;
        rest = tail;
    }
    write_frame(socket, id, typ, rest, fd)
}

/// One frame: the header from a stack buffer and the body in place, so a
/// large body is not copied to be sent.
fn write_frame(socket: i32, id: u64, typ: u32, body: &[u8], fd: Option<i32>) -> io::Result<()> {
    // `send_message` keeps every body within `MAX_BODY`, so this fits a `u32`.
    let length = u32::try_from(12 + body.len()).unwrap_or(u32::MAX);
    let mut header = [0u8; 16];
    for (slot, byte) in header.iter_mut().zip(
        length
            .to_le_bytes()
            .into_iter()
            .chain(typ.to_le_bytes())
            .chain(id.to_le_bytes()),
    ) {
        *slot = byte;
    }
    send_slices(socket, [&header, body], fd)
}

/// Sends both slices in order, retrying partial writes. `fd` goes with the
/// first byte.
pub(crate) fn send_slices(socket: i32, slices: [&[u8]; 2], fd: Option<i32>) -> io::Result<()> {
    let [head, body] = slices;
    let total = head.len() + body.len();
    let mut sent = 0;
    while sent < total {
        let head_rest = head.get(sent..).unwrap_or_default();
        let body_rest = body
            .get(sent.saturating_sub(head.len())..)
            .unwrap_or_default();
        let mut iov = [head_rest, body_rest].map(|slice| libc::iovec {
            iov_base: slice.as_ptr().cast_mut().cast(),
            iov_len: slice.len(),
        });
        let mut control = [0u32; crate::control::cmsg_space_fd() / 4];
        let pass = if sent == 0 { fd } else { None };
        let mut msg = libc::msghdr {
            msg_name: std::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: iov.as_mut_ptr(),
            msg_iovlen: 2,
            msg_control: std::ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: 0,
        };
        if let Some(fd) = pass {
            crate::control::write_rights(&mut control, fd);
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = crate::control::cmsg_space_fd() as libc::socklen_t;
        }
        // SAFETY: `msg` points at two live iovecs and, when passing a
        // descriptor, at a control buffer sized for one `SCM_RIGHTS` header.
        let n = unsafe { libc::sendmsg(socket, &msg, 0) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
        if n == 0 {
            return Err(io::Error::new(
                ErrorKind::WriteZero,
                "control socket accepted no bytes",
            ));
        }
        sent += n as usize;
    }
    Ok(())
}
