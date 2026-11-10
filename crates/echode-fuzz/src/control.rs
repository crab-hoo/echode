//! The control socket, read the way the host reads a helper's replies and
//! notices (`echode_sandbox::Socket`).
//!
//! Input: one mode byte, a little-endian `u16` split point, then the stream.
//! The bytes before the split are sent plainly. The rest is sent in one
//! `sendmsg` that carries `mode % 4` descriptors, so a descriptor can arrive
//! at any message boundary or inside a message.
//!
//! The descriptors are copies of one pipe's write end. Once the sockets are
//! closed, the read end reaches its end only if no copy is left open in this
//! process, which other threads' files cannot disturb (as in ADR 0288).

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::fd::FromRawFd;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use echode_sandbox::{
    AudioPort, ControlMessage, GuiReport, LaunchFailure, LaunchFailureKind, MainReplyKind,
    ParamChange, ParamEvent, ParamEventKind, ParamInfo, ParamRelay, ParamText, ParamValue, Pending,
    PluginCommand, PluginFailure, Refusal, SessionLayout, Socket,
};

use crate::bytes::Bytes;

/// Longer streams are skipped. Bounds this target's socket buffers only; the
/// product accepts messages up to 64 MiB.
const MAX_STREAM: usize = 256 * 1024;
const BUFFER: i32 = 1024 * 1024;

pub(crate) fn run(data: &[u8]) {
    let mut input = Bytes::new(data);
    let rights = usize::from(input.u8() % 4);
    let split = usize::from(input.u16());
    let stream = input.take(usize::MAX);
    if stream.len() > MAX_STREAM {
        return;
    }
    let (mut ends, write_end) = pipe();
    read_stream(stream, split.min(stream.len()), &write_end, rights);
    drop(write_end);
    let mut byte = [0u8];
    match ends.read(&mut byte) {
        Ok(0) => {}
        Ok(_) => unreachable!("nothing writes to the pipe"),
        Err(error) => {
            assert_ne!(
                error.kind(),
                ErrorKind::WouldBlock,
                "the control socket left a received descriptor open"
            );
            panic!("read the pipe: {error}");
        }
    }
}

/// A non-blocking read end and its write end.
fn pipe() -> (File, File) {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` has room for both descriptors.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
    // SAFETY: `pipe` returned two open descriptors this function now owns.
    let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
    // SAFETY: `read` is open; the flags are plain integers.
    let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0, "fcntl");
    // SAFETY: as above.
    let rc = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    assert_eq!(rc, 0, "fcntl");
    (read, write)
}

fn read_stream(stream: &[u8], split: usize, write_end: &File, rights: usize) {
    let (mut writer, reader) = UnixStream::pair().expect("socket pair");
    enlarge_buffers(writer.as_raw_fd());
    enlarge_buffers(reader.as_raw_fd());
    let mut socket = Socket::from_owned(OwnedFd::from(reader)).expect("control socket");
    let (plain, carried) = stream.split_at(split);
    writer.write_all(plain).expect("write the stream");
    send_with_rights(&writer, carried, write_end, rights);
    writer.shutdown(Shutdown::Write).expect("end the stream");
    // Every receive consumes bytes or fails, so this ends; the bound only
    // guards the target against a receive that neither reads nor fails.
    for _ in 0..=stream.len() + 2 {
        match socket.try_recv_tagged() {
            Ok(Some((id, message))) => assert_round_trip(id, message),
            Ok(None) => {}
            // The host closes a helper's connection after the first error.
            Err(_) => return,
        }
    }
    panic!("the control socket kept receiving without reading");
}

/// A message the host decoded encodes to the same message.
fn assert_round_trip(id: u64, message: ControlMessage) {
    let (mut sender, mut receiver) = Socket::pair().expect("socket pair");
    enlarge_buffers(sender.as_raw_fd());
    enlarge_buffers(receiver.as_raw_fd());
    let expected = describe(&message);
    match message {
        ControlMessage::Remap { layout, fd } => sender.send_remap_tagged(id, layout, fd),
        other => sender.send_tagged(id, &other),
    }
    .expect("a decoded control message encodes again");
    let (again_id, again) = receiver
        .recv_tagged(Duration::from_secs(5))
        .expect("an encoded control message decodes");
    assert_eq!(again_id, id, "the request id changed in a round trip");
    assert_eq!(
        describe(&again),
        expected,
        "a control message changed in a round trip"
    );
}

/// `Debug`, except that a remap's descriptor number is left out.
fn describe(message: &ControlMessage) -> String {
    match message {
        ControlMessage::Remap { layout, .. } => format!("Remap({layout:?})"),
        other => format!("{other:?}"),
    }
}

fn send_with_rights(writer: &UnixStream, bytes: &[u8], write_end: &File, rights: usize) {
    if bytes.is_empty() {
        return;
    }
    let files: Vec<File> = (0..rights)
        .map(|_| write_end.try_clone().expect("copy the pipe's write end"))
        .collect();
    let fds: Vec<i32> = files.iter().map(AsRawFd::as_raw_fd).collect();
    let data_len = std::mem::size_of_val(fds.as_slice());
    // SAFETY: CMSG_SPACE only computes a size.
    let space = unsafe { libc::CMSG_SPACE(data_len as u32) } as usize;
    // `u32` words give the header the 4-byte alignment Darwin uses.
    let mut control = vec![0u32; space.div_ceil(4)];
    let mut sent = 0;
    while sent < bytes.len() {
        let rest = &bytes[sent..];
        let mut iov = libc::iovec {
            iov_base: rest.as_ptr().cast_mut().cast(),
            iov_len: rest.len(),
        };
        let mut msg = libc::msghdr {
            msg_name: std::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &mut iov,
            msg_iovlen: 1,
            msg_control: std::ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: 0,
        };
        if sent == 0 && !fds.is_empty() {
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = space as libc::socklen_t;
            // SAFETY: the control buffer is CMSG_SPACE bytes for `fds`, so the
            // first header and its data lie inside it.
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&msg);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(data_len as u32) as libc::socklen_t;
                std::ptr::copy_nonoverlapping(
                    fds.as_ptr().cast::<u8>(),
                    libc::CMSG_DATA(header),
                    data_len,
                );
            }
        }
        // SAFETY: `msg` points at live buffers for the duration of the call.
        let n = unsafe { libc::sendmsg(writer.as_raw_fd(), &msg, 0) };
        if n < 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(error.kind(), ErrorKind::Interrupted, "sendmsg: {error}");
            continue;
        }
        sent += n as usize;
    }
}

fn enlarge_buffers(fd: i32) {
    let size = BUFFER;
    for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
        // SAFETY: `fd` is an open socket and the value is a live `i32`.
        let rc = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&raw const size).cast(),
                std::mem::size_of::<i32>() as libc::socklen_t,
            )
        };
        assert_eq!(rc, 0, "setsockopt: {}", std::io::Error::last_os_error());
    }
}

pub(crate) fn seeds() -> Vec<Vec<u8>> {
    let mut messages = sample_messages();
    messages.extend(launch_failures());
    use MainReplyKind::*;
    for expected in [State, Done, Parameters, Values, Parsed, Window] {
        for actual in [State, Done, Parameters, Values, Parsed, Window] {
            messages.push(ControlMessage::StateRejected(
                Refusal::UnexpectedMainReply { expected, actual },
            ));
        }
    }
    for id in [0, u32::MAX - 1, u32::MAX] {
        messages.push(ControlMessage::ParamsRejected(Refusal::InvalidParameterId(
            id,
        )));
    }
    let mut seeds = Vec::new();
    let mut all = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let bytes = encode(index as u64 + 1, message);
        seeds.push(seed(0, 0, &bytes));
        all.extend_from_slice(&bytes);
    }
    seeds.push(seed(0, 0, &all));
    // A remap with its mapping, after one plain message.
    let hello = encode(0, &ControlMessage::Hello);
    let mut stream = hello.clone();
    stream.extend_from_slice(&encode_remap(7, SessionLayout::stereo()));
    seeds.push(seed(1, hello.len() as u16, &stream));
    seeds
}

fn launch_failures() -> impl Iterator<Item = ControlMessage> {
    use LaunchFailureKind::*;
    [
        Library,
        InvalidNulPath { position: u64::MAX },
        EntryIo { raw_os_error: None },
        EntryIo {
            raw_os_error: Some(libc::EACCES),
        },
        IncompatibleClapVersion {
            plugin: [u32::MAX, 0, 1],
            host: [1, 2, 3],
        },
        InstantiationFailed,
        ActivationFailed,
    ]
    .into_iter()
    .map(|kind| {
        ControlMessage::PortsRejected(Refusal::Launch(LaunchFailure {
            kind,
            diagnostic: "native loader diagnostic — 音".to_string(),
        }))
    })
}

fn seed(rights: u8, split: u16, stream: &[u8]) -> Vec<u8> {
    let mut bytes = vec![rights];
    bytes.extend_from_slice(&split.to_le_bytes());
    bytes.extend_from_slice(stream);
    bytes
}

/// The bytes the product's sender writes for one message.
fn encode(id: u64, message: &ControlMessage) -> Vec<u8> {
    let (sender, mut raw) = sender_pair();
    let mut sender = sender;
    sender
        .send_tagged(id, message)
        .expect("send a seed message");
    drop(sender);
    let mut bytes = Vec::new();
    raw.read_to_end(&mut bytes).expect("read a seed message");
    bytes
}

fn encode_remap(id: u64, layout: SessionLayout) -> Vec<u8> {
    let (mut sender, mut raw) = sender_pair();
    let mapping = OwnedFd::from(File::open("/dev/null").expect("open /dev/null"));
    sender
        .send_remap_tagged(id, layout, mapping)
        .expect("send a seed remap");
    drop(sender);
    let mut bytes = Vec::new();
    raw.read_to_end(&mut bytes).expect("read a seed remap");
    bytes
}

fn sender_pair() -> (Socket, UnixStream) {
    let (sender, raw) = UnixStream::pair().expect("socket pair");
    enlarge_buffers(sender.as_raw_fd());
    enlarge_buffers(raw.as_raw_fd());
    let sender = Socket::from_owned(OwnedFd::from(sender)).expect("control socket");
    (sender, raw)
}

fn sample_messages() -> Vec<ControlMessage> {
    let value = ParamValue {
        id: 3,
        value: 0.25,
        text: "-12 dB".to_string(),
    };
    vec![
        ControlMessage::Hello,
        ControlMessage::SaveRequest,
        ControlMessage::State(b"plugin state".to_vec()),
        ControlMessage::Restore(b"plugin state".to_vec()),
        ControlMessage::Restored,
        ControlMessage::RequestProcess,
        ControlMessage::Dirty,
        ControlMessage::PluginRestarted {
            latency_samples: 64,
        },
        ControlMessage::Remapped,
        ControlMessage::RemapFailed,
        ControlMessage::CancelRemap,
        ControlMessage::RemapCancelled,
        ControlMessage::SetChain(3),
        ControlMessage::ChainReady,
        ControlMessage::ChainRejected,
        ControlMessage::Ports {
            inputs: vec![2],
            outputs: vec![2, 1],
            tail_samples: 480,
            latency_samples: 32,
        },
        ControlMessage::PortsRejected(Refusal::Other("no audio ports".to_string())),
        ControlMessage::PortsRejected(Refusal::Plugin(PluginFailure::PortWithoutChannels(
            AudioPort {
                input: false,
                index: 1,
            },
        ))),
        ControlMessage::StateRejected(Refusal::Other("refused".to_string())),
        ControlMessage::StateRejected(Refusal::Plugin(PluginFailure::NoState)),
        ControlMessage::StateRejected(Refusal::TimedOut(Pending::MainCall(9))),
        ControlMessage::StateRejected(Refusal::Stopped(Pending::MainCall(10))),
        ControlMessage::ParamsRequest,
        ControlMessage::Params(vec![ParamInfo {
            id: 3,
            flags: 1,
            name: "Gain".to_string(),
            module: "Output/Main".to_string(),
            min: -60.0,
            max: 12.0,
            default: 0.0,
            value: -12.0,
            text: "-12 dB".to_string(),
        }]),
        ControlMessage::SetParams(vec![ParamChange { id: 3, value: 0.5 }]),
        ControlMessage::ParamsApplied(vec![value.clone()]),
        ControlMessage::ParamsRejected(Refusal::Other("closed".to_string())),
        ControlMessage::ParamsRejected(Refusal::Busy),
        ControlMessage::ParamsRejected(Refusal::NoPlugin),
        ControlMessage::ParamsRejected(Refusal::TimedOut(Pending::ParameterChanges)),
        ControlMessage::ParamsRejected(Refusal::Stopped(Pending::ParameterChanges)),
        ControlMessage::ParseParam {
            id: 3,
            text: "-6 dB".to_string(),
        },
        ControlMessage::ParamParsed(ParamText::Value(-6.0)),
        ControlMessage::ParamParsed(ParamText::Refused),
        ControlMessage::ParamParsed(ParamText::Unsupported),
        ControlMessage::GuiShow("Synth — Lead".to_string()),
        ControlMessage::GuiShown(GuiReport {
            created: true,
            visible: true,
            key: false,
        }),
        ControlMessage::GuiClose,
        ControlMessage::GuiClosedReply,
        ControlMessage::GuiRejected(Refusal::Other("no window".to_string())),
        ControlMessage::GuiRejected(Refusal::Plugin(PluginFailure::NoWindow)),
        ControlMessage::GuiClosed,
        ControlMessage::ParamRelay(ParamRelay {
            events: vec![
                ParamEvent {
                    id: 3,
                    kind: ParamEventKind::Begin,
                },
                ParamEvent {
                    id: 3,
                    kind: ParamEventKind::Value,
                },
            ],
            values: vec![value],
            rescan: false,
            lost: true,
        }),
        ControlMessage::GuiCommand(PluginCommand::Undo),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_left_open_keeps_the_pipe_from_ending() {
        let (mut read, write) = pipe();
        let copy = write.try_clone().unwrap();
        drop(write);
        let error = read.read(&mut [0]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::WouldBlock);
        drop(copy);
        assert_eq!(read.read(&mut [0]).unwrap(), 0);
    }
}
