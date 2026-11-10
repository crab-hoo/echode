//! Control socket. The audio thread never reads or writes it.
//!
//! Messages are a little-endian length, a type, a request id, and a body. `Remap` also carries
//! one file descriptor with `SCM_RIGHTS`. A body longer than 64 MiB is rejected
//! so a corrupt peer cannot force an unbounded allocation.

use crate::control_text::{ControlTextError, read_text};
use crate::gui::{GuiReport, ParamRelay, read_relay, read_report, write_relay, write_report};
use crate::params::{
    ParamChange, ParamInfo, ParamText, ParamValue, read_changes, read_infos, read_parsed,
    read_text_request, read_values, write_changes, write_infos, write_parsed, write_text_request,
    write_values,
};
use crate::refusal::{Refusal, read_refusal, write_refusal};
use crate::region::SessionLayout;
use std::io::{self, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// Ancillary data one `recvmsg` can hold: 2,048 bytes. Darwin refuses longer
/// ancillary data on send (`MCLBYTES`) and, measured, more than 254 descriptors
/// in one message (1,028 bytes received). The kernel installs every passed
/// descriptor even when the buffer truncates them, so a smaller buffer would
/// leave descriptors open that this process never sees (ADR 0288).
const RECEIVE_CONTROL_WORDS: usize = 512;

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
const PARAMS_REQUEST: u32 = 20;
const PARAMS: u32 = 21;
const SET_PARAMS: u32 = 22;
const PARAMS_APPLIED: u32 = 23;
const PARAMS_REJECTED: u32 = 24;
const GUI_SHOW: u32 = 25;
const GUI_SHOWN: u32 = 26;
const GUI_CLOSE: u32 = 27;
const GUI_CLOSED_REPLY: u32 = 28;
const GUI_REJECTED: u32 = 29;
const GUI_CLOSED: u32 = 30;
const PARAM_RELAY: u32 = 31;
const GUI_COMMAND: u32 = 32;
const PARSE_PARAM: u32 = 33;
const PARAM_PARSED: u32 = 34;

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
    /// The helper sends port counts with tail and latency after activation.
    /// The counts themselves are queried on the main thread while inactive.
    Ports {
        inputs: Vec<u32>,
        outputs: Vec<u32>,
        tail_samples: u32,
        latency_samples: u32,
    },
    /// The plugin has no usable audio-port description.
    PortsRejected(Refusal),
    /// Save or load did not finish: the helper did not call the plugin, the
    /// plugin refused, or the outcome is unknown.
    StateRejected(Refusal),
    /// Asks for every parameter description and current value.
    ParamsRequest,
    /// The plugin's parameters in its own order. A plugin without the params
    /// extension sends an empty list.
    Params(Vec<ParamInfo>),
    /// Values to apply in order on the plugin's processing thread.
    SetParams(Vec<ParamChange>),
    /// The changed parameters, read back after the plugin applied them.
    ParamsApplied(Vec<ParamValue>),
    /// A parameter request did not finish in the plugin.
    ParamsRejected(Refusal),
    /// Asks the plugin to read typed text for one parameter. Changes nothing.
    ParseParam {
        id: u32,
        text: String,
    },
    /// The plugin's reading of the text, or that it has no converter.
    ParamParsed(ParamText),
    /// Opens the plugin window, or raises it when it is open. Never takes focus.
    /// The bytes are the window title.
    GuiShow(String),
    GuiShown(GuiReport),
    /// Closes the plugin window and destroys the plugin's view.
    GuiClose,
    GuiClosedReply,
    /// The window could not be shown or closed.
    GuiRejected(Refusal),
    /// Notice: the user closed the plugin window.
    GuiClosed,
    /// Notice: values and gestures the plugin reported on its own.
    ParamRelay(ParamRelay),
    /// Notice: a global DAW shortcut from the focused plugin window.
    GuiCommand(crate::PluginCommand),
}

impl ControlMessage {
    pub fn is_notice(&self) -> bool {
        matches!(
            self,
            Self::Dirty
                | Self::PluginRestarted { .. }
                | Self::RequestProcess
                | Self::Hello
                | Self::GuiClosed
                | Self::ParamRelay(_)
                | Self::GuiCommand(_)
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
    /// Set when the peer broke the descriptor rules. That read's bytes were
    /// dropped and every later receive fails, so the message is never accepted.
    broken: Option<&'static str>,
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
            broken: None,
        })
    }

    pub fn send(&mut self, message: &ControlMessage) -> io::Result<()> {
        self.send_tagged(0, message)
    }

    /// A nonzero id correlates one control request and its reply. Notices use 0.
    pub fn send_tagged(&mut self, id: u64, message: &ControlMessage) -> io::Result<()> {
        let (typ, body) = encode(message)?;
        let bytes = frame_tagged(id, typ, &body)?;
        send_bytes(self.fd.as_raw_fd(), &bytes, None)
    }

    /// Send a new mapping. `fd` is duplicated by the kernel; this end is closed.
    pub fn send_remap(&mut self, layout: SessionLayout, fd: OwnedFd) -> io::Result<()> {
        self.send_remap_tagged(0, layout, fd)
    }

    pub fn send_remap_tagged(
        &mut self,
        id: u64,
        layout: SessionLayout,
        fd: OwnedFd,
    ) -> io::Result<()> {
        let body = layout_bytes(layout);
        let bytes = frame_tagged(id, REMAP, &body)?;
        send_bytes(self.fd.as_raw_fd(), &bytes, Some(fd.as_raw_fd()))?;
        drop(fd);
        Ok(())
    }

    pub fn wait_readable(&self, timeout: Duration) -> io::Result<bool> {
        poll_read(self.fd.as_raw_fd(), timeout)
    }

    pub fn try_recv(&mut self) -> io::Result<Option<ControlMessage>> {
        Ok(self.try_recv_tagged()?.map(|(_, message)| message))
    }

    pub fn try_recv_tagged(&mut self) -> io::Result<Option<(u64, ControlMessage)>> {
        if let Some(message) = self.parse_one()? {
            return Ok(Some(message));
        }
        if !self.read_some(true)? {
            return Ok(None);
        }
        self.parse_one()
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> io::Result<ControlMessage> {
        self.recv_tagged(timeout).map(|(_, message)| message)
    }

    pub fn recv_tagged(&mut self, timeout: Duration) -> io::Result<(u64, ControlMessage)> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.try_recv_tagged()? {
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

    fn parse_one(&mut self) -> io::Result<Option<(u64, ControlMessage)>> {
        let Some(header) = self.incoming.first_chunk::<4>() else {
            return Ok(None);
        };
        let len = u32::from_le_bytes(*header) as usize;
        if !(12..=MAX_MESSAGE).contains(&len) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "control message length is not usable",
            ));
        }
        let total = 4 + len;
        if self.incoming.len() < total {
            return Ok(None);
        }
        let mut payload: Vec<u8> = self.incoming.drain(..total).skip(4).collect();
        let fd = match self.pending_fd.take() {
            Some(pending) if pending.at < total => Some(pending.fd),
            Some(mut pending) => {
                pending.at -= total;
                self.pending_fd = Some(pending);
                None
            }
            None => None,
        };
        // `len` is at least 12, so the type and the id are both present.
        let id = payload
            .get(4..12)
            .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
            .ok_or_else(|| {
                io::Error::new(ErrorKind::InvalidData, "control message has no request id")
            })?;
        let id = u64::from_le_bytes(id);
        payload.drain(4..12);
        Ok(decode(&payload, fd)?.map(|message| (id, message)))
    }

    fn read_some(&mut self, nonblock: bool) -> io::Result<bool> {
        if let Some(reason) = self.broken {
            return Err(io::Error::new(ErrorKind::InvalidData, reason));
        }
        let mut buf = [0u8; 8192];
        // `u32` words give the ancillary headers the 4-byte alignment Darwin uses.
        let mut control = [0u32; RECEIVE_CONTROL_WORDS];
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
            msg_controllen: std::mem::size_of_val(&control) as libc::socklen_t,
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
        // Every received descriptor is owned before any check, so a rejected
        // read closes them all (SBX-A1 (2), ADR 0288).
        let mut fds = take_rights(&msg);
        let violation = if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            Some("control data was truncated")
        } else if fds.len() > 1 {
            Some("a control message carried more than one file descriptor")
        } else if !fds.is_empty() && self.pending_fd.is_some() {
            Some("a second file descriptor arrived before the first was consumed")
        } else {
            None
        };
        if let Some(reason) = violation {
            self.broken = Some(reason);
            return Err(io::Error::new(ErrorKind::InvalidData, reason));
        }
        let at = self.incoming.len();
        self.incoming
            .extend_from_slice(buf.get(..n as usize).unwrap_or_default());
        if let Some(fd) = fds.pop() {
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
        ControlMessage::Ports {
            inputs,
            outputs,
            tail_samples,
            latency_samples,
        } => {
            let mut bytes = port_bytes(inputs, outputs);
            bytes.extend_from_slice(&tail_samples.to_le_bytes());
            bytes.extend_from_slice(&latency_samples.to_le_bytes());
            (PORTS, bytes)
        }
        ControlMessage::PortsRejected(refusal) => (PORTS_REJECTED, write_refusal(refusal)),
        ControlMessage::StateRejected(refusal) => (STATE_REJECTED, write_refusal(refusal)),
        ControlMessage::ParamsRequest => (PARAMS_REQUEST, Vec::new()),
        ControlMessage::Params(infos) => (PARAMS, write_infos(infos)),
        ControlMessage::SetParams(changes) => (SET_PARAMS, write_changes(changes)),
        ControlMessage::ParamsApplied(values) => (PARAMS_APPLIED, write_values(values)),
        ControlMessage::ParamsRejected(refusal) => (PARAMS_REJECTED, write_refusal(refusal)),
        ControlMessage::ParseParam { id, text } => (PARSE_PARAM, write_text_request(*id, text)),
        ControlMessage::ParamParsed(parsed) => (PARAM_PARSED, write_parsed(*parsed)),
        ControlMessage::GuiShow(title) => (GUI_SHOW, title.as_bytes().to_vec()),
        ControlMessage::GuiShown(report) => (GUI_SHOWN, write_report(*report)),
        ControlMessage::GuiClose => (GUI_CLOSE, Vec::new()),
        ControlMessage::GuiClosedReply => (GUI_CLOSED_REPLY, Vec::new()),
        ControlMessage::GuiRejected(refusal) => (GUI_REJECTED, write_refusal(refusal)),
        ControlMessage::GuiClosed => (GUI_CLOSED, Vec::new()),
        ControlMessage::GuiCommand(command) => (GUI_COMMAND, vec![*command as u8]),
        ControlMessage::ParamRelay(relay) => (PARAM_RELAY, write_relay(relay)),
    })
}

fn decode(payload: &[u8], fd: Option<OwnedFd>) -> io::Result<Option<ControlMessage>> {
    let Some((typ, body)) = payload.split_first_chunk::<4>() else {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "control message has no type",
        ));
    };
    let typ = u32::from_le_bytes(*typ);
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
            let missing = || io::Error::new(ErrorKind::InvalidData, "port tail/latency is missing");
            let (rest, latency) = body.split_last_chunk::<4>().ok_or_else(missing)?;
            let (ports, tail) = rest.split_last_chunk::<4>().ok_or_else(missing)?;
            let (inputs, outputs) = read_ports(ports)?;
            ControlMessage::Ports {
                inputs,
                outputs,
                tail_samples: u32::from_le_bytes(*tail),
                latency_samples: u32::from_le_bytes(*latency),
            }
        }
        PORTS_REJECTED => ControlMessage::PortsRejected(read_refusal(body)?),
        STATE_REJECTED => ControlMessage::StateRejected(read_refusal(body)?),
        PARAMS_REQUEST if body.is_empty() => ControlMessage::ParamsRequest,
        PARAMS => ControlMessage::Params(read_infos(body)?),
        SET_PARAMS => ControlMessage::SetParams(read_changes(body)?),
        PARAMS_APPLIED => ControlMessage::ParamsApplied(read_values(body)?),
        PARAMS_REJECTED => ControlMessage::ParamsRejected(read_refusal(body)?),
        PARSE_PARAM => {
            let (id, text) = read_text_request(body)?;
            ControlMessage::ParseParam { id, text }
        }
        PARAM_PARSED => ControlMessage::ParamParsed(read_parsed(body)?),
        GUI_SHOW => ControlMessage::GuiShow(read_text(body, ControlTextError::WindowTitle)?),
        GUI_SHOWN => ControlMessage::GuiShown(read_report(body)?),
        GUI_CLOSE if body.is_empty() => ControlMessage::GuiClose,
        GUI_CLOSED_REPLY if body.is_empty() => ControlMessage::GuiClosedReply,
        GUI_REJECTED => ControlMessage::GuiRejected(read_refusal(body)?),
        GUI_CLOSED if body.is_empty() => ControlMessage::GuiClosed,
        GUI_COMMAND if body.len() == 1 => ControlMessage::GuiCommand(
            body.first()
                .copied()
                .and_then(crate::PluginCommand::decode)
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown plugin command"))?,
        ),
        PARAM_RELAY => ControlMessage::ParamRelay(read_relay(body)?),
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
    let truncated = || io::Error::new(ErrorKind::InvalidData, "port description is truncated");
    let (count, rest) = body.split_first_chunk::<4>().ok_or_else(truncated)?;
    let count = u32::from_le_bytes(*count) as usize;
    let bytes = count
        .checked_mul(4)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "port description is too large"))?;
    let (ports, rest) = rest.split_at_checked(bytes).ok_or_else(truncated)?;
    let counts = ports
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect();
    Ok((counts, rest))
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
    for (word, field) in body.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
        *word = field.to_le_bytes();
    }
    body
}

fn read_layout(body: &[u8]) -> io::Result<SessionLayout> {
    let (
        [
            max_frames,
            input_channels,
            output_channels,
            input_events,
            output_events,
        ],
        [],
    ) = body.as_chunks::<4>()
    else {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "remap layout is the wrong size",
        ));
    };
    Ok(SessionLayout {
        max_frames: u32::from_le_bytes(*max_frames),
        input_channels: u32::from_le_bytes(*input_channels),
        output_channels: u32::from_le_bytes(*output_channels),
        input_events: u32::from_le_bytes(*input_events),
        output_events: u32::from_le_bytes(*output_events),
    })
}

fn frame_tagged(id: u64, typ: u32, body: &[u8]) -> io::Result<Vec<u8>> {
    let len = 12 + body.len();
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "control message is longer than 64 MiB",
        ));
    }
    let mut bytes = Vec::with_capacity(4 + len);
    bytes.extend_from_slice(&(len as u32).to_le_bytes());
    bytes.extend_from_slice(&typ.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(body);
    Ok(bytes)
}

fn send_bytes(socket: i32, bytes: &[u8], fd: Option<i32>) -> io::Result<()> {
    let mut sent = 0;
    while sent < bytes.len() {
        let rest = bytes.get(sent..).unwrap_or_default();
        let mut iov = libc::iovec {
            iov_base: rest.as_ptr().cast_mut().cast(),
            iov_len: rest.len(),
        };
        let mut control = [0u32; cmsg_space_fd() / 4];
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
const fn cmsg_align(len: usize) -> usize {
    len.saturating_add(3) & !3
}

const fn cmsg_space_fd() -> usize {
    cmsg_align(std::mem::size_of::<libc::cmsghdr>()) + cmsg_align(std::mem::size_of::<i32>())
}

fn cmsg_len_fd() -> libc::socklen_t {
    (cmsg_align(std::mem::size_of::<libc::cmsghdr>()) + std::mem::size_of::<i32>())
        as libc::socklen_t
}

fn write_rights(buf: &mut [u32], fd: i32) {
    debug_assert!(std::mem::size_of_val(buf) >= cmsg_space_fd());
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

/// Owns every descriptor one `recvmsg` delivered, so the caller closes any the
/// protocol does not use by dropping them. Darwin refuses to send a second
/// ancillary header on a Unix socket (EINVAL), but each header is read anyway.
fn take_rights(msg: &libc::msghdr) -> Vec<OwnedFd> {
    let data_offset = cmsg_align(std::mem::size_of::<libc::cmsghdr>());
    let end = (msg.msg_control as usize).saturating_add(msg.msg_controllen as usize);
    let mut fds = Vec::new();
    // SAFETY: `msg` is the header `recvmsg` filled. `CMSG_FIRSTHDR` and
    // `CMSG_NXTHDR` return null rather than a header past `msg_controllen`.
    let mut header = unsafe { libc::CMSG_FIRSTHDR(msg) };
    while !header.is_null() {
        // SAFETY: `header` points at a whole `cmsghdr` inside the control buffer.
        let (level, typ, len) = unsafe {
            (
                (*header).cmsg_level,
                (*header).cmsg_type,
                (*header).cmsg_len as usize,
            )
        };
        if len < data_offset {
            // A length that does not cover its own header would repeat it forever.
            break;
        }
        if level == libc::SOL_SOCKET && typ == libc::SCM_RIGHTS {
            // A truncated header's length can pass the end of what was copied.
            let copied = len.min(end.saturating_sub(header as usize));
            let count = copied.saturating_sub(data_offset) / std::mem::size_of::<i32>();
            // SAFETY: the data follows this header inside the control buffer.
            let data = unsafe { libc::CMSG_DATA(header) };
            for slot in 0..count {
                // SAFETY: slot `count - 1` ends within the copied control data.
                let fd = unsafe {
                    std::ptr::read_unaligned(
                        data.add(slot * std::mem::size_of::<i32>()).cast::<i32>(),
                    )
                };
                if fd < 0 {
                    continue;
                }
                // SAFETY: `recvmsg` transferred this descriptor to this process.
                let owned = unsafe { OwnedFd::from_raw_fd(fd) };
                let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFD) };
                if flags >= 0 {
                    unsafe {
                        libc::fcntl(owned.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC)
                    };
                }
                fds.push(owned);
            }
        }
        // SAFETY: as for `CMSG_FIRSTHDR`; `header` came from this `msg`.
        header = unsafe { libc::CMSG_NXTHDR(msg, header) };
    }
    fds
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

    #[test]
    fn window_messages_cross_the_socket() {
        use crate::gui::{ParamEvent, ParamEventKind};
        let (mut parent, mut child) = Socket::pair().expect("socketpair");
        parent
            .send(&ControlMessage::GuiShow("Bass — Test".into()))
            .expect("show");
        assert!(matches!(
            child.recv_timeout(Duration::from_secs(2)).expect("recv"),
            ControlMessage::GuiShow(title) if title == "Bass — Test"
        ));
        let report = GuiReport {
            created: true,
            visible: true,
            key: false,
        };
        child
            .send(&ControlMessage::GuiShown(report))
            .expect("shown");
        let relay = ParamRelay {
            events: vec![ParamEvent {
                id: 2,
                kind: ParamEventKind::End,
            }],
            ..ParamRelay::default()
        };
        child
            .send(&ControlMessage::ParamRelay(relay.clone()))
            .expect("relay");
        child.send(&ControlMessage::GuiClosed).expect("closed");
        assert!(matches!(
            parent.recv_timeout(Duration::from_secs(2)).expect("recv"),
            ControlMessage::GuiShown(got) if got == report
        ));
        let notice = parent.recv_timeout(Duration::from_secs(2)).expect("recv");
        assert!(notice.is_notice());
        assert!(matches!(notice, ControlMessage::ParamRelay(got) if got == relay));
        assert!(matches!(
            parent.recv_timeout(Duration::from_secs(2)).expect("recv"),
            ControlMessage::GuiClosed
        ));
        let (kind, _) = encode(&ControlMessage::GuiClose).unwrap();
        let mut payload = kind.to_le_bytes().to_vec();
        payload.push(0);
        assert!(
            decode(&payload, None).is_err(),
            "a close request has no body"
        );
    }

    #[test]
    fn parameter_messages_cross_the_socket() {
        let (mut parent, mut child) = Socket::pair().expect("socketpair");
        let change = ParamChange { id: 3, value: 0.75 };
        parent
            .send(&ControlMessage::SetParams(vec![change]))
            .expect("set");
        let message = child.recv_timeout(Duration::from_secs(2)).expect("recv");
        assert!(matches!(message, ControlMessage::SetParams(changes) if changes == [change]));
        let applied = ParamValue {
            id: 3,
            value: 0.75,
            text: "75 %".into(),
        };
        child
            .send(&ControlMessage::ParamsApplied(vec![applied.clone()]))
            .expect("applied");
        let reply = parent.recv_timeout(Duration::from_secs(2)).expect("reply");
        assert!(matches!(reply, ControlMessage::ParamsApplied(values) if values == [applied]));
        let info = ParamInfo {
            id: 3,
            flags: 0,
            name: "Mix".into(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            value: 0.75,
            text: "75 %".into(),
        };
        child
            .send(&ControlMessage::Params(vec![info.clone()]))
            .expect("params");
        let reply = parent.recv_timeout(Duration::from_secs(2)).expect("reply");
        assert!(matches!(reply, ControlMessage::Params(infos) if infos == [info]));
        parent
            .send(&ControlMessage::ParseParam {
                id: 3,
                text: "40 %".into(),
            })
            .expect("parse");
        assert!(matches!(
            child.recv_timeout(Duration::from_secs(2)).expect("recv"),
            ControlMessage::ParseParam { id: 3, text } if text == "40 %"
        ));
        child
            .send(&ControlMessage::ParamParsed(ParamText::Value(0.4)))
            .expect("parsed");
        assert!(matches!(
            parent.recv_timeout(Duration::from_secs(2)).expect("reply"),
            ControlMessage::ParamParsed(ParamText::Value(value)) if value == 0.4
        ));
        let (kind, _) = encode(&ControlMessage::ParamsRequest).unwrap();
        let mut payload = kind.to_le_bytes().to_vec();
        payload.push(1);
        assert!(decode(&payload, None).is_err());
    }

    #[test]
    fn port_metadata_round_trips_the_declared_tail_and_latency() {
        let message = ControlMessage::Ports {
            inputs: vec![],
            outputs: vec![1],
            tail_samples: 19_200,
            latency_samples: 256,
        };
        let (kind, bytes) = encode(&message).unwrap();
        let mut payload = kind.to_le_bytes().to_vec();
        payload.extend(bytes);
        assert!(
            matches!(decode(&payload, None).unwrap(), Some(ControlMessage::Ports { inputs, outputs, tail_samples: 19_200, latency_samples: 256 }) if inputs.is_empty() && outputs == [1])
        );
        payload.pop();
        assert!(decode(&payload, None).is_err());
    }
}

#[cfg(test)]
#[path = "control_tagged_tests.rs"]
mod tagged_tests;

#[cfg(test)]
#[path = "control_rights_tests.rs"]
mod rights_tests;

#[cfg(test)]
#[path = "control_text_tests.rs"]
mod text_tests;

#[cfg(test)]
#[path = "control_launch_tests.rs"]
mod launch_tests;
