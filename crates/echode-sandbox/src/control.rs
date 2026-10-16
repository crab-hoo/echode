//! Control socket. The audio thread never reads or writes it.
//!
//! Messages are a little-endian length, a type, and a body. `Remap` also carries
//! one file descriptor with `SCM_RIGHTS`. A body longer than 64 MiB is rejected
//! so a corrupt peer cannot force an unbounded allocation.

use crate::region::SessionLayout;
use std::io::{self, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

const MAX_MESSAGE: usize = 64 * 1024 * 1024;

const HELLO: u32 = 1;
const SAVE: u32 = 2;
const STATE: u32 = 3;
const RESTORE: u32 = 4;
const RESTORED: u32 = 5;
const REQUEST_PROCESS: u32 = 6;
const DIRTY: u32 = 7;
const PLUGIN_RESTARTED: u32 = 8;
const REMAP: u32 = 9;
const REMAPPED: u32 = 10;
const REMAP_FAILED: u32 = 11;
const SET_CHAIN: u32 = 12;
const CHAIN_READY: u32 = 13;
const CHAIN_REJECTED: u32 = 14;
const PORTS: u32 = 15;
const PORTS_REJECTED: u32 = 16;
const STATE_REJECTED: u32 = 17;
const CANCEL_REMAP: u32 = 18;
const REMAP_CANCELLED: u32 = 19;

#[derive(Debug)]
pub enum ControlMessage {
    Hello,
    SaveRequest,
    State(Vec<u8>),
    Restore(Vec<u8>),
    Restored,
    RequestProcess,
    Dirty,
    PluginRestarted {
        latency_samples: u32,
    },
    Remap {
        layout: SessionLayout,
        fd: OwnedFd,
    },
    Remapped,
    RemapFailed,
    CancelRemap,
    RemapCancelled,
    /// How many stages this helper runs, in order, for one submitted block.
    SetChain(u32),
    ChainReady,
    ChainRejected,
    /// Channel counts of each input port, then each output port.
    ///
    /// The helper sends this after reading the plugin's audio-ports extension
    /// on the main thread, before the plugin is active.
    Ports {
        inputs: Vec<u32>,
        outputs: Vec<u32>,
    },
    /// The plugin has no usable audio-port description. The bytes are the reason.
    PortsRejected(String),
    /// Save or load did not call the plugin, or the plugin refused. The bytes are the reason.
    StateRejected(String),
}

impl ControlMessage {
    pub fn is_notice(&self) -> bool {
        matches!(
            self,
            Self::Dirty | Self::PluginRestarted { .. } | Self::RequestProcess | Self::Hello
        )
    }
}

struct PendingFd {
    /// Index in `incoming` of the first byte this descriptor belongs to.
    at: usize,
    fd: OwnedFd,
}

pub struct Socket {
    fd: OwnedFd,
    incoming: Vec<u8>,
    pending_fd: Option<PendingFd>,
}

impl Socket {
    pub fn pair() -> io::Result<(Self, Self)> {
        let mut fds = [0i32; 2];
        let rc = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `socketpair` returned two open descriptors.
        let parent = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let child = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        Ok((Self::from_owned(parent)?, Self::from_owned(child)?))
    }

    pub fn as_raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    pub fn from_owned(fd: OwnedFd) -> io::Result<Self> {
        configure(fd.as_raw_fd())?;
        Ok(Self {
            fd,
            incoming: Vec::new(),
            pending_fd: None,
        })
    }

    pub fn send(&mut self, message: &ControlMessage) -> io::Result<()> {
        let (typ, body) = encode(message)?;
        let bytes = frame(typ, &body)?;
        send_bytes(self.fd.as_raw_fd(), &bytes, None)
    }

    /// Send a new mapping. `fd` is duplicated by the kernel; this end is closed.
    pub fn send_remap(&mut self, layout: SessionLayout, fd: OwnedFd) -> io::Result<()> {
        let body = layout_bytes(layout);
        let bytes = frame(REMAP, &body)?;
        send_bytes(self.fd.as_raw_fd(), &bytes, Some(fd.as_raw_fd()))?;
        drop(fd);
        Ok(())
    }

    pub fn wait_readable(&self, timeout: Duration) -> io::Result<bool> {
        poll_read(self.fd.as_raw_fd(), timeout)
    }

    pub fn try_recv(&mut self) -> io::Result<Option<ControlMessage>> {
        if let Some(message) = self.parse_one()? {
            return Ok(Some(message));
        }
        if !self.read_some(true)? {
            return Ok(None);
        }
        self.parse_one()
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> io::Result<ControlMessage> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.try_recv()? {
                return Ok(message);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    ErrorKind::TimedOut,
                    "control message was not received before the deadline",
                ));
            }
            if !poll_read(self.fd.as_raw_fd(), remaining)? && self.incoming.len() < 4 {
                continue;
            }
        }
    }

    fn parse_one(&mut self) -> io::Result<Option<ControlMessage>> {
        if self.incoming.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.incoming[..4].try_into().expect("4 bytes")) as usize;
        if !(4..=MAX_MESSAGE).contains(&len) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "control message length is not usable",
            ));
        }
        let total = 4 + len;
        if self.incoming.len() < total {
            return Ok(None);
        }
        let payload: Vec<u8> = self.incoming.drain(..total).skip(4).collect();
        let fd = match self.pending_fd.take() {
            Some(pending) if pending.at < total => Some(pending.fd),
            Some(mut pending) => {
                pending.at -= total;
                self.pending_fd = Some(pending);
                None
            }
            None => None,
        };
        decode(&payload, fd)
    }

    fn read_some(&mut self, nonblock: bool) -> io::Result<bool> {
        let mut buf = [0u8; 8192];
        let mut control = [0u8; 256];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        let mut msg = libc::msghdr {
            msg_name: std::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &mut iov,
            msg_iovlen: 1,
            msg_control: control.as_mut_ptr().cast(),
            msg_controllen: control.len() as libc::socklen_t,
            msg_flags: 0,
        };
        let flags = if nonblock { libc::MSG_DONTWAIT } else { 0 };
        let n = unsafe { libc::recvmsg(self.fd.as_raw_fd(), &mut msg, flags) };
        if n < 0 {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => return self.read_some(nonblock),
                Some(libc::EAGAIN) => return Ok(false),
                _ if error.kind() == ErrorKind::WouldBlock => return Ok(false),
                _ => return Err(error),
            }
        }
        if n == 0 {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "control socket closed",
            ));
        }
        let at = self.incoming.len();
        self.incoming.extend_from_slice(&buf[..n as usize]);
        if let Some(fd) = take_rights(&msg) {
            if self.pending_fd.is_some() {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "a second file descriptor arrived before the first was consumed",
                ));
            }
            self.pending_fd = Some(PendingFd { at, fd });
        }
        Ok(true)
    }
}

fn encode(message: &ControlMessage) -> io::Result<(u32, Vec<u8>)> {
    Ok(match message {
        ControlMessage::Hello => (HELLO, Vec::new()),
        ControlMessage::SaveRequest => (SAVE, Vec::new()),
        ControlMessage::State(bytes) => (STATE, bytes.clone()),
        ControlMessage::Restore(bytes) => (RESTORE, bytes.clone()),
        ControlMessage::Restored => (RESTORED, Vec::new()),
        ControlMessage::RequestProcess => (REQUEST_PROCESS, Vec::new()),
        ControlMessage::Dirty => (DIRTY, Vec::new()),
        ControlMessage::PluginRestarted { latency_samples } => {
            (PLUGIN_RESTARTED, latency_samples.to_le_bytes().to_vec())
        }
        ControlMessage::Remap { .. } => {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "remap carries a file descriptor",
            ));
        }
        ControlMessage::Remapped => (REMAPPED, Vec::new()),
        ControlMessage::RemapFailed => (REMAP_FAILED, Vec::new()),
        ControlMessage::CancelRemap => (CANCEL_REMAP, Vec::new()),
        ControlMessage::RemapCancelled => (REMAP_CANCELLED, Vec::new()),
        ControlMessage::SetChain(stages) => (SET_CHAIN, stages.to_le_bytes().to_vec()),
        ControlMessage::ChainReady => (CHAIN_READY, Vec::new()),
        ControlMessage::ChainRejected => (CHAIN_REJECTED, Vec::new()),
        ControlMessage::Ports { inputs, outputs } => (PORTS, port_bytes(inputs, outputs)),
        ControlMessage::PortsRejected(reason) => (PORTS_REJECTED, reason.as_bytes().to_vec()),
        ControlMessage::StateRejected(reason) => (STATE_REJECTED, reason.as_bytes().to_vec()),
    })
}

fn decode(payload: &[u8], fd: Option<OwnedFd>) -> io::Result<Option<ControlMessage>> {
    if payload.len() < 4 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "control message has no type",
        ));
    }
    let typ = u32::from_le_bytes(payload[..4].try_into().expect("4 bytes"));
    let body = &payload[4..];
    if typ == REMAP {
        let layout = read_layout(body)?;
        let fd = fd.ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidData, "remap did not include a mapping")
        })?;
        return Ok(Some(ControlMessage::Remap { layout, fd }));
    }
    drop(fd);
    let message = match typ {
        HELLO => ControlMessage::Hello,
        SAVE => ControlMessage::SaveRequest,
        STATE => ControlMessage::State(body.to_vec()),
        RESTORE => ControlMessage::Restore(body.to_vec()),
        RESTORED => ControlMessage::Restored,
        REQUEST_PROCESS => ControlMessage::RequestProcess,
        DIRTY => ControlMessage::Dirty,
        PLUGIN_RESTARTED => {
            let bytes: [u8; 4] = body.try_into().map_err(|_| {
                io::Error::new(ErrorKind::InvalidData, "restart notice is the wrong size")
            })?;
            ControlMessage::PluginRestarted {
                latency_samples: u32::from_le_bytes(bytes),
            }
        }
        REMAPPED => ControlMessage::Remapped,
        REMAP_FAILED => ControlMessage::RemapFailed,
        CANCEL_REMAP => ControlMessage::CancelRemap,
        REMAP_CANCELLED => ControlMessage::RemapCancelled,
        SET_CHAIN => {
            let bytes: [u8; 4] = body.try_into().map_err(|_| {
                io::Error::new(ErrorKind::InvalidData, "chain message is the wrong size")
            })?;
            ControlMessage::SetChain(u32::from_le_bytes(bytes))
        }
        CHAIN_READY => ControlMessage::ChainReady,
        CHAIN_REJECTED => ControlMessage::ChainRejected,
        PORTS => {
            let (inputs, outputs) = read_ports(body)?;
            ControlMessage::Ports { inputs, outputs }
        }
        PORTS_REJECTED => ControlMessage::PortsRejected(
            String::from_utf8(body.to_vec())
                .map_err(|_| io::Error::new(ErrorKind::InvalidData, "port refusal is not text"))?,
        ),
        STATE_REJECTED => ControlMessage::StateRejected(
            String::from_utf8(body.to_vec())
                .map_err(|_| io::Error::new(ErrorKind::InvalidData, "state refusal is not text"))?,
        ),
        _ => {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "unknown control message",
            ));
        }
    };
    Ok(Some(message))
}

fn port_bytes(inputs: &[u32], outputs: &[u32]) -> Vec<u8> {
    let mut body = Vec::with_capacity((2 + inputs.len() + outputs.len()) * 4);
    write_counts(&mut body, inputs);
    write_counts(&mut body, outputs);
    body
}

fn write_counts(body: &mut Vec<u8>, counts: &[u32]) {
    let count = u32::try_from(counts.len()).unwrap_or(u32::MAX);
    body.extend_from_slice(&count.to_le_bytes());
    for channels in counts.iter().take(count as usize) {
        body.extend_from_slice(&channels.to_le_bytes());
    }
}

fn read_ports(body: &[u8]) -> io::Result<(Vec<u32>, Vec<u32>)> {
    let (inputs, rest) = read_counts(body)?;
    let (outputs, rest) = read_counts(rest)?;
    if !rest.is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "port description has trailing bytes",
        ));
    }
    Ok((inputs, outputs))
}

fn read_counts(body: &[u8]) -> io::Result<(Vec<u32>, &[u8])> {
    if body.len() < 4 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "port description is truncated",
        ));
    }
    let count = u32::from_le_bytes(body[..4].try_into().expect("4 bytes")) as usize;
    let bytes = count
        .checked_mul(4)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "port description is too large"))?;
    let rest = body
        .get(4..)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "port description is truncated"))?;
    if rest.len() < bytes {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "port description is truncated",
        ));
    }
    let mut counts = Vec::with_capacity(count);
    for chunk in rest[..bytes].chunks_exact(4) {
        let channels = u32::from_le_bytes(chunk.try_into().expect("4 bytes"));
        counts.push(channels);
    }
    Ok((counts, &rest[bytes..]))
}

fn layout_bytes(layout: SessionLayout) -> [u8; 20] {
    let mut body = [0u8; 20];
    let fields = [
        layout.max_frames,
        layout.input_channels,
        layout.output_channels,
        layout.input_events,
        layout.output_events,
    ];
    for (index, field) in fields.into_iter().enumerate() {
        body[index * 4..index * 4 + 4].copy_from_slice(&field.to_le_bytes());
    }
    body
}

fn read_layout(body: &[u8]) -> io::Result<SessionLayout> {
    if body.len() != 20 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "remap layout is the wrong size",
        ));
    }
    let word = |index: usize| {
        u32::from_le_bytes(body[index * 4..index * 4 + 4].try_into().expect("4 bytes"))
    };
    Ok(SessionLayout {
        max_frames: word(0),
        input_channels: word(1),
        output_channels: word(2),
        input_events: word(3),
        output_events: word(4),
    })
}

fn frame(typ: u32, body: &[u8]) -> io::Result<Vec<u8>> {
    let len = 4 + body.len();
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "control message is longer than 64 MiB",
        ));
    }
    let mut bytes = Vec::with_capacity(4 + len);
    bytes.extend_from_slice(&(len as u32).to_le_bytes());
    bytes.extend_from_slice(&typ.to_le_bytes());
    bytes.extend_from_slice(body);
    Ok(bytes)
}

fn send_bytes(socket: i32, bytes: &[u8], fd: Option<i32>) -> io::Result<()> {
    let mut sent = 0;
    while sent < bytes.len() {
        let mut iov = libc::iovec {
            iov_base: bytes[sent..].as_ptr().cast_mut().cast(),
            iov_len: bytes.len() - sent,
        };
        let mut control = [0u8; 256];
        let pass = if sent == 0 { fd } else { None };
        let mut msg = libc::msghdr {
            msg_name: std::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &mut iov,
            msg_iovlen: 1,
            msg_control: std::ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: 0,
        };
        if let Some(fd) = pass {
            write_rights(&mut control, fd);
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = cmsg_space_fd() as libc::socklen_t;
        }
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

fn poll_read(fd: i32, timeout: Duration) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = i32::try_from(timeout.as_millis())
        .unwrap_or(i32::MAX)
        .max(1);
    let rc = unsafe { libc::poll(&mut poll, 1, millis) };
    if rc < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(rc > 0)
}

fn configure(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let one = 1i32;
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&raw const one).cast(),
            std::mem::size_of::<i32>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Darwin aligns ancillary headers to 4 bytes (`__DARWIN_ALIGN32`).
fn cmsg_align(len: usize) -> usize {
    len.saturating_add(3) & !3
}

fn cmsg_space_fd() -> usize {
    cmsg_align(std::mem::size_of::<libc::cmsghdr>()) + cmsg_align(std::mem::size_of::<i32>())
}

fn cmsg_len_fd() -> libc::socklen_t {
    (cmsg_align(std::mem::size_of::<libc::cmsghdr>()) + std::mem::size_of::<i32>())
        as libc::socklen_t
}

fn write_rights(buf: &mut [u8], fd: i32) {
    debug_assert!(buf.len() >= cmsg_space_fd());
    let header = buf.as_mut_ptr().cast::<libc::cmsghdr>();
    // SAFETY: `buf` is large enough for one `SCM_RIGHTS` header and one fd.
    unsafe {
        (*header).cmsg_len = cmsg_len_fd();
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        let data = (header.cast::<u8>()).add(cmsg_align(std::mem::size_of::<libc::cmsghdr>()));
        std::ptr::write_unaligned(data.cast::<i32>(), fd);
    }
}

fn take_rights(msg: &libc::msghdr) -> Option<OwnedFd> {
    // SAFETY: `msg` is the `recvmsg` header for this call. A null header means
    // the datagram carried no descriptor.
    let header = unsafe { libc::CMSG_FIRSTHDR(msg) };
    if header.is_null() {
        return None;
    }
    let (level, typ) = unsafe { ((*header).cmsg_level, (*header).cmsg_type) };
    if level != libc::SOL_SOCKET || typ != libc::SCM_RIGHTS {
        return None;
    }
    let fd = unsafe {
        let data = header
            .cast::<u8>()
            .add(cmsg_align(std::mem::size_of::<libc::cmsghdr>()));
        std::ptr::read_unaligned(data.cast::<i32>())
    };
    if fd < 0 {
        return None;
    }
    // SAFETY: `recvmsg` transferred this descriptor to us.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFD) };
    if flags >= 0 {
        unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    }
    Some(owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::MappedRegion;

    #[test]
    fn remap_passes_a_mapping() {
        let (mut parent, mut child) = Socket::pair().expect("socketpair");
        let layout = SessionLayout {
            max_frames: 32,
            input_channels: 0,
            output_channels: 1,
            input_events: 2,
            output_events: 2,
        };
        let region = MappedRegion::create_shared(layout).expect("shm");
        let dup = region.duplicate_fd().expect("dup");
        parent.send_remap(layout, dup).expect("send");
        let message = child.recv_timeout(Duration::from_secs(2)).expect("recv");
        let ControlMessage::Remap { layout: got, fd } = message else {
            panic!("expected remap");
        };
        assert_eq!(got, layout);
        let mapped = MappedRegion::from_fd(fd).expect("map");
        assert_eq!(mapped.layout(), layout);
        child.send(&ControlMessage::Remapped).expect("ack");
        let ack = parent.recv_timeout(Duration::from_secs(2)).expect("ack");
        assert!(matches!(ack, ControlMessage::Remapped));
    }

    #[test]
    fn state_bytes_round_trip() {
        let (mut parent, mut child) = Socket::pair().expect("socketpair");
        parent
            .send(&ControlMessage::Restore(b"abc".to_vec()))
            .expect("restore");
        let message = child.recv_timeout(Duration::from_secs(2)).expect("recv");
        assert!(matches!(message, ControlMessage::Restore(bytes) if bytes == b"abc"));
    }
}
