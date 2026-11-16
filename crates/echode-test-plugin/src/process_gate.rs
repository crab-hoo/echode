//! Hold a fixture's processing without socket I/O on its audio thread.
//!
//! The relay owns the blocking handshake; processing only publishes a ticket
//! and spins until the test releases it. This intentionally misses a deadline,
//! but remains valid under the helper's RealtimeSanitizer instrumentation.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Default)]
struct State {
    requested: AtomicUsize,
    released: AtomicUsize,
    stopped: AtomicBool,
}

pub(crate) struct ProcessGate {
    state: Arc<State>,
    socket: UnixStream,
    relay: Option<JoinHandle<()>>,
    ticket: usize,
}

impl ProcessGate {
    /// Called during activation, before the audio processor starts.
    pub(crate) fn connect(path: &str) -> Self {
        Self::new(UnixStream::connect(path).expect("process gate"))
    }

    fn new(socket: UnixStream) -> Self {
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("process gate timeout");
        let mut peer = socket.try_clone().expect("process gate relay");
        let state = Arc::new(State::default());
        let shared = Arc::clone(&state);
        let relay = std::thread::spawn(move || {
            let mut ticket = 0;
            while !shared.stopped.load(Ordering::Acquire) {
                if shared.requested.load(Ordering::Acquire) == ticket {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                if peer.write_all(&[1]).is_err() || peer.read_exact(&mut [0]).is_err() {
                    break;
                }
                ticket += 1;
                shared.released.store(ticket, Ordering::Release);
                if peer.write_all(&[2]).is_err() {
                    break;
                }
            }
            // A closed test peer or teardown must also release a spinning call.
            shared.stopped.store(true, Ordering::Release);
        });
        Self {
            state,
            socket,
            relay: Some(relay),
            ticket: 0,
        }
    }

    pub(crate) fn hold(&mut self) {
        self.ticket += 1;
        self.state.requested.store(self.ticket, Ordering::Release);
        while self.state.released.load(Ordering::Acquire) < self.ticket
            && !self.state.stopped.load(Ordering::Acquire)
        {
            std::hint::spin_loop();
        }
    }
}

impl Drop for ProcessGate {
    fn drop(&mut self) {
        // Deactivation destroys the processor off RT. Shutdown also wakes a
        // relay whose test ended without sending its release byte.
        self.state.stopped.store(true, Ordering::Release);
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(relay) = self.relay.take() {
            relay.join().expect("process gate relay panicked");
        }
    }
}

#[cfg(test)]
#[path = "process_gate_tests.rs"]
mod tests;
