//! Descriptors a peer attaches beyond the protocol's one per message (SBX-A1 (2), ADR 0288).

use super::*;

/// A pipe whose read end sees end of file only once every copy of its write
/// end is closed, in this process and in flight on a socket.
struct Witness {
    read: OwnedFd,
    write: Option<OwnedFd>,
}

impl Witness {
    fn new() -> Self {
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
        for fd in fds {
            assert!(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } >= 0);
        }
        assert!(unsafe { libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK) } >= 0);
        // SAFETY: `pipe` returned two open descriptors.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Self {
            read,
            write: Some(write),
        }
    }

    fn copies(&self, count: usize) -> Vec<OwnedFd> {
        let write = self.write.as_ref().expect("write end").as_raw_fd();
        (0..count)
            .map(|_| {
                let fd = unsafe { libc::fcntl(write, libc::F_DUPFD_CLOEXEC, 0) };
                assert!(fd >= 0, "dup: {}", io::Error::last_os_error());
                // SAFETY: `fcntl` returned a new descriptor this test owns.
                unsafe { OwnedFd::from_raw_fd(fd) }
            })
            .collect()
    }

    /// Closes this test's write end. True when no other copy is open.
    fn all_closed(mut self) -> bool {
        self.write = None;
        let mut byte = 0u8;
        let n = unsafe { libc::read(self.read.as_raw_fd(), (&raw mut byte).cast(), 1) };
        n == 0
    }
}

/// Sends `bytes` with `fds` in one `SCM_RIGHTS` header, as a peer may.
fn send_with_rights(socket: &Socket, bytes: &[u8], fds: &[OwnedFd]) -> io::Result<()> {
    let header_len = cmsg_align(std::mem::size_of::<libc::cmsghdr>());
    let total = header_len + fds.len() * std::mem::size_of::<i32>();
    // `u32` words give the header the 4-byte alignment Darwin uses.
    let mut control = vec![0u32; total.div_ceil(4)];
    let base = control.as_mut_ptr().cast::<u8>();
    // SAFETY: `control` holds the header and every descriptor slot.
    unsafe {
        let header = base.cast::<libc::cmsghdr>();
        (*header).cmsg_len = total as libc::socklen_t;
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        for (slot, fd) in fds.iter().enumerate() {
            let at = base.add(header_len + slot * std::mem::size_of::<i32>());
            std::ptr::write_unaligned(at.cast::<i32>(), fd.as_raw_fd());
        }
    }
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr().cast_mut().cast(),
        iov_len: bytes.len(),
    };
    let msg = libc::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: base.cast(),
        msg_controllen: total as libc::socklen_t,
        msg_flags: 0,
    };
    let n = unsafe { libc::sendmsg(socket.as_raw_fd(), &msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    assert_eq!(n as usize, bytes.len(), "short send");
    Ok(())
}

fn remap_frame() -> Vec<u8> {
    // One frame and one channel size this fixture's body, not a product limit.
    let layout = SessionLayout {
        max_frames: 1,
        input_channels: 0,
        output_channels: 1,
        input_events: 1,
        output_events: 1,
    };
    frame_tagged(0, REMAP, &layout_bytes(layout)).unwrap()
}

/// What one delivery left behind.
struct Delivery {
    first: Option<ErrorKind>,
    /// A later receive, which must not accept the rejected bytes.
    later: io::Result<Option<ControlMessage>>,
    closed: bool,
}

/// Sends one frame with these descriptors, closes the sender's copies, and
/// receives twice. The witness is read while the receiver is still open, so a
/// descriptor it kept in any form, pending or leaked, keeps it open.
fn deliver(frame: &[u8], fds: Vec<OwnedFd>, witness: Witness) -> io::Result<Delivery> {
    let (sender, mut receiver) = Socket::pair()?;
    send_with_rights(&sender, frame, &fds)?;
    drop(fds);
    let first = receiver
        .recv_timeout(Duration::from_secs(2))
        .err()
        .map(|error| error.kind());
    let later = receiver.try_recv();
    let closed = witness.all_closed();
    drop(receiver);
    Ok(Delivery {
        first,
        later,
        closed,
    })
}

/// Darwin's measured ceiling for one `SCM_RIGHTS` message.
const MOST_DESCRIPTORS: usize = 254;

#[test]
fn contract_received_descriptors_beyond_one_are_rejected_and_all_closed() {
    // Up to 2 × 254 descriptors are open at once below.
    crate::descriptors::raise_descriptor_limit();
    let notice = frame_tagged(0, DIRTY, &[]).unwrap();
    for (name, frame) in [("remap", remap_frame()), ("notice", notice)] {
        for count in [2, 3, MOST_DESCRIPTORS] {
            let witness = Witness::new();
            let fds = witness.copies(count);
            let delivery = deliver(&frame, fds, witness).unwrap();
            assert_eq!(
                delivery.first,
                Some(ErrorKind::InvalidData),
                "a {name} with {count} descriptors breaks the protocol"
            );
            assert!(
                matches!(&delivery.later, Err(error) if error.kind() == ErrorKind::InvalidData),
                "the rejected {name} is not accepted later: {:?}",
                delivery.later
            );
            assert!(
                delivery.closed,
                "all {count} descriptors of a {name} are closed"
            );
        }
    }
}

#[test]
fn one_descriptor_still_arrives_with_its_remap() {
    let witness = Witness::new();
    let (sender, mut receiver) = Socket::pair().unwrap();
    let fds = witness.copies(1);
    send_with_rights(&sender, &remap_frame(), &fds).unwrap();
    drop(fds);
    let ControlMessage::Remap { fd, .. } = receiver.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected remap");
    };
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    assert!(
        flags >= 0 && flags & libc::FD_CLOEXEC != 0,
        "received descriptors close on exec"
    );
    drop(fd);
    assert!(witness.all_closed());
}

/// The receive buffer is sized on the premise that the kernel refuses any
/// message whose descriptors would not fit in it. If this starts sending, the
/// kernel could install descriptors past the buffer that nothing closes.
#[test]
fn the_kernel_refuses_more_descriptors_than_the_receive_buffer_holds() {
    crate::descriptors::raise_descriptor_limit();
    let header = cmsg_align(std::mem::size_of::<libc::cmsghdr>());
    let fits = (RECEIVE_CONTROL_WORDS * 4 - header) / std::mem::size_of::<i32>();
    let witness = Witness::new();
    let (sender, _receiver) = Socket::pair().unwrap();
    let fds = witness.copies(fits + 1);
    let error = send_with_rights(&sender, &remap_frame(), &fds).unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(libc::EINVAL),
        "{} descriptors",
        fits + 1
    );
    drop(fds);
    assert!(witness.all_closed());
}

#[test]
fn a_second_descriptor_before_the_first_is_used_is_rejected_and_closed() {
    let (first_witness, second_witness) = (Witness::new(), Witness::new());
    let (sender, mut receiver) = Socket::pair().unwrap();
    let frame = remap_frame();
    let (head, tail) = frame.split_at(6);
    let first = first_witness.copies(1);
    send_with_rights(&sender, head, &first).unwrap();
    drop(first);
    assert!(
        receiver.try_recv().unwrap().is_none(),
        "the frame is partial"
    );
    let second = second_witness.copies(1);
    send_with_rights(&sender, tail, &second).unwrap();
    drop(second);
    let error = receiver.recv_timeout(Duration::from_secs(2)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert!(receiver.try_recv().is_err(), "the socket stays rejected");
    assert!(
        second_witness.all_closed(),
        "the second descriptor is closed"
    );
    drop(receiver);
    assert!(
        first_witness.all_closed(),
        "the pending one closes with the socket"
    );
}
