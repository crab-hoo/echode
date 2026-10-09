//! Parent side: spawn `echode-plugin-host` and exchange blocks with it.
//!
//! The mapping is a shared-memory object whose name is unlinked immediately.
//! The child receives that descriptor and a control socket. The audio exchange
//! never touches the socket. A `kqueue` on the control side notices when the
//! helper exits.

use crate::control::{ControlMessage, Socket};
use crate::region::{MappedRegion, RegionError, SessionLayout};
use crate::sync::{SyncFlags, wake_one};
use crate::workgroup::spawn_helper;
use crate::{CMD_PROCESS, CMD_SHUTDOWN};
use echode_realtime::Event;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const STDERR_CAP: usize = 64 * 1024;

#[derive(Debug)]
pub struct SpawnOptions<'a> {
    pub host_bin: &'a Path,
    pub plugin: Option<&'a Path>,
    /// Passed through as `ECHODE_TEST_FAULT` (`crash` or `hang`).
    pub fault: Option<&'a str>,
    /// When set, the helper is spawned with this workgroup send right.
    pub workgroup: Option<WorkgroupHandoff>,
    pub layout: SessionLayout,
}

/// A send right from `os_workgroup_copy_port` and the device period, in mach ticks.
#[derive(Debug)]
pub struct WorkgroupHandoff {
    pub port: u32,
    pub period_ticks: u32,
}

/// What the helper tells the parent without a reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginNotice {
    Dirty,
    PluginRestarted { latency_samples: u32 },
    RequestProcess,
}

struct HelperProcess {
    pid: i32,
    status: Option<ExitStatus>,
}

struct Launched {
    child: HelperProcess,
    stderr: Box<dyn Read + Send>,
    control: Socket,
}

struct WatchSlots {
    notice: *const AtomicU64,
    dead: *const AtomicU64,
}

// SAFETY: the pointers address atomics in a mapping this process keeps alive
// while the watcher holds `WatchSlots`, and the mutex serializes that use.
unsafe impl Send for WatchSlots {}

pub struct PluginHost {
    region: MappedRegion,
    child: HelperProcess,
    control: Socket,
    inbox: VecDeque<ControlMessage>,
    stderr: Arc<Mutex<String>>,
    closed: bool,
    last_state: Vec<u8>,
    host_bin: PathBuf,
    plugin: Option<PathBuf>,
    watch: Option<JoinHandle<()>>,
    watch_slots: Arc<Mutex<WatchSlots>>,
    watch_stop: Arc<AtomicBool>,
}

#[derive(Debug)]
pub enum SessionError {
    Region(RegionError),
    Io(std::io::Error),
    Timeout,
    Missed,
    HostExited { status: ExitStatus, stderr: String },
    Workgroup(String),
    Protocol(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Region(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Timeout => f.write_str("plugin process did not answer before the deadline"),
            Self::Missed => f.write_str("both block slots are still in flight"),
            Self::Workgroup(error) => write!(f, "{error}"),
            Self::Protocol(error) => write!(f, "{error}"),
            Self::HostExited { status, stderr } => {
                write!(f, "plugin process exited ({status})")?;
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SessionError {}

impl From<RegionError> for SessionError {
    fn from(error: RegionError) -> Self {
        match error {
            RegionError::Wait(crate::sync::WaitError::Timeout) => Self::Timeout,
            RegionError::Busy => Self::Missed,
            other => Self::Region(other),
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl PluginHost {
    pub fn spawn(mut options: SpawnOptions<'_>) -> Result<Self, SessionError> {
        let region = MappedRegion::create_shared(options.layout)?;
        let shm = region.as_raw_fd();
        let launched = launch(
            options.host_bin,
            options.plugin,
            shm,
            options.workgroup.take(),
            options.fault,
        )?;
        let stderr = Arc::new(Mutex::new(String::new()));
        spawn_stderr_reader(launched.stderr, Arc::clone(&stderr));
        let watch_slots = Arc::new(Mutex::new(WatchSlots {
            notice: region.notice_ptr(),
            dead: region.dead_ptr(),
        }));
        let watch_stop = Arc::new(AtomicBool::new(false));
        let mut host = Self {
            region,
            child: launched.child,
            control: launched.control,
            inbox: VecDeque::new(),
            stderr,
            closed: false,
            last_state: Vec::new(),
            host_bin: options.host_bin.to_path_buf(),
            plugin: options.plugin.map(Path::to_path_buf),
            watch: None,
            watch_slots,
            watch_stop,
        };
        host.start_watch();
        match host.wait_until_ready(Duration::from_secs(5)) {
            Ok(()) => Ok(host),
            Err(error) => {
                host.force_stop();
                Err(error)
            }
        }
    }

    /// Attach a new helper to the same mapping after resetting its header.
    /// The shared-memory name stays unlinked; the child receives the existing fd.
    pub fn restart(&mut self) -> Result<(), SessionError> {
        self.stop_watch();
        if self.child.try_wait()?.is_none() {
            self.force_stop();
        }
        self.region.reset_for_restart();
        self.point_watch();
        self.inbox.clear();
        self.closed = false;
        let launched = launch(
            &self.host_bin,
            self.plugin.as_deref(),
            self.region.as_raw_fd(),
            None,
            None,
        )?;
        spawn_stderr_reader(launched.stderr, Arc::clone(&self.stderr));
        self.child = launched.child;
        self.control = launched.control;
        self.start_watch();
        self.wait_until_ready(Duration::from_secs(5))?;
        if !self.last_state.is_empty() {
            let state = self.last_state.clone();
            self.restore_state(&state)?;
        }
        Ok(())
    }

    pub fn stderr_log(&self) -> String {
        self.stderr
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }

    pub fn process(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        timeout: Duration,
    ) -> Result<(), SessionError> {
        self.exchange(CMD_PROCESS, input, output, timeout)
    }

    pub fn process_events(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        events_in: &[Event],
        events_out: &mut [Event],
        timeout: Duration,
    ) -> Result<usize, SessionError> {
        let frames = if !input.is_empty() {
            input[0].len()
        } else if !output.is_empty() {
            output[0].len()
        } else {
            return Err(RegionError::BadShape {
                frames: 0,
                channels: 0,
            }
            .into());
        };
        self.exchange_events(
            frames,
            input,
            output,
            events_in,
            events_out,
            CMD_PROCESS,
            timeout,
        )
    }

    /// `frames` is required when the session has no audio channels.
    pub fn process_block(
        &mut self,
        frames: usize,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        events_in: &[Event],
        events_out: &mut [Event],
        timeout: Duration,
    ) -> Result<usize, SessionError> {
        self.exchange_events(
            frames,
            input,
            output,
            events_in,
            events_out,
            CMD_PROCESS,
            timeout,
        )
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, SessionError> {
        self.control.send(&ControlMessage::SaveRequest)?;
        match self.wait_message(Duration::from_secs(2), |message| {
            matches!(message, ControlMessage::State(_))
        })? {
            ControlMessage::State(bytes) => {
                self.last_state.clone_from(&bytes);
                Ok(bytes)
            }
            _ => Err(SessionError::Protocol(
                "helper did not return state".to_string(),
            )),
        }
    }

    pub fn restore_state(&mut self, bytes: &[u8]) -> Result<(), SessionError> {
        self.control
            .send(&ControlMessage::Restore(bytes.to_vec()))?;
        match self.wait_message(Duration::from_secs(2), |message| {
            matches!(message, ControlMessage::Restored)
        })? {
            ControlMessage::Restored => {
                self.last_state = bytes.to_vec();
                Ok(())
            }
            _ => Err(SessionError::Protocol(
                "helper did not acknowledge restore".to_string(),
            )),
        }
    }

    pub fn poll_notice(&mut self) -> Result<Option<PluginNotice>, SessionError> {
        self.drain()?;
        let index = self.inbox.iter().position(|message| {
            matches!(
                message,
                ControlMessage::Dirty
                    | ControlMessage::PluginRestarted { .. }
                    | ControlMessage::RequestProcess
            )
        });
        let Some(index) = index else {
            return Ok(None);
        };
        Ok(Some(match self.inbox.remove(index) {
            Some(ControlMessage::Dirty) => PluginNotice::Dirty,
            Some(ControlMessage::PluginRestarted { latency_samples }) => {
                PluginNotice::PluginRestarted { latency_samples }
            }
            Some(ControlMessage::RequestProcess) => PluginNotice::RequestProcess,
            _ => {
                return Err(SessionError::Protocol(
                    "queued notice disappeared".to_string(),
                ));
            }
        }))
    }

    /// Install a new mapping between blocks. A failed lock keeps the old one.
    pub fn remap(&mut self, layout: SessionLayout) -> Result<(), SessionError> {
        let region = MappedRegion::create_shared(layout)?;
        let duplicate = region.duplicate_fd()?;
        self.control.send_remap(layout, duplicate)?;
        match self.wait_message(Duration::from_secs(2), |message| {
            matches!(
                message,
                ControlMessage::Remapped | ControlMessage::RemapFailed
            )
        })? {
            ControlMessage::Remapped => {
                self.install_region(region);
                Ok(())
            }
            ControlMessage::RemapFailed => Err(SessionError::Protocol(
                "helper kept the previous mapping".to_string(),
            )),
            _ => Err(SessionError::Protocol(
                "helper did not answer remap".to_string(),
            )),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), SessionError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if self.child.try_wait()?.is_none() {
            let _ = self.send_shutdown();
            if self.child.try_wait()?.is_none() {
                self.force_stop();
            }
        }
        self.stop_watch();
        Ok(())
    }

    fn send_shutdown(&mut self) -> Result<(), SessionError> {
        let layout = self.region.layout();
        let frames = 1;
        let inputs = vec![vec![0.0f32; frames]; layout.input_channels as usize];
        let mut outputs = vec![vec![0.0f32; frames]; layout.output_channels as usize];
        let input_refs: Vec<&[f32]> = inputs.iter().map(|plane| plane.as_slice()).collect();
        let mut output_refs: Vec<&mut [f32]> = outputs
            .iter_mut()
            .map(|plane| plane.as_mut_slice())
            .collect();
        let mut none = [];
        self.exchange_events(
            frames,
            &input_refs,
            &mut output_refs,
            &[],
            &mut none,
            CMD_SHUTDOWN,
            Duration::from_millis(200),
        )
        .map(|_| ())
    }

    fn exchange(
        &mut self,
        command: u32,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        timeout: Duration,
    ) -> Result<(), SessionError> {
        match self.region.request(0, input, output, command, timeout) {
            Ok(()) => Ok(()),
            Err(error) => Err(self.fail(error)),
        }
    }

    #[allow(clippy::too_many_arguments)] // planes, events, command, and the deadline are one block
    fn exchange_events(
        &mut self,
        frames: usize,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        events_in: &[Event],
        events_out: &mut [Event],
        command: u32,
        timeout: Duration,
    ) -> Result<usize, SessionError> {
        match self.region.request_events(
            frames, input, output, events_in, events_out, command, timeout,
        ) {
            Ok(count) => Ok(count),
            Err(error) => Err(self.fail(error)),
        }
    }

    fn fail(&mut self, error: RegionError) -> SessionError {
        match error {
            RegionError::Wait(crate::sync::WaitError::Timeout) => self.fail_timeout(),
            RegionError::Dead => self.reap_dead(),
            RegionError::Busy => SessionError::Missed,
            other => other.into(),
        }
    }

    fn wait_until_ready(&mut self, timeout: Duration) -> Result<(), SessionError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(self.exited(status));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SessionError::Timeout);
            }
            let slice = remaining.min(Duration::from_millis(20));
            match self.region.wait_ready(slice) {
                Ok(()) => return Ok(()),
                Err(RegionError::Wait(crate::sync::WaitError::Timeout)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn fail_timeout(&mut self) -> SessionError {
        match self.child.try_wait() {
            Ok(Some(status)) => self.exited(status),
            Ok(None) => SessionError::Timeout,
            Err(error) => SessionError::Io(error),
        }
    }

    fn reap_dead(&mut self) -> SessionError {
        match self.child.try_wait() {
            Ok(Some(status)) => self.exited(status),
            Ok(None) => match self.child.wait() {
                Ok(status) => self.exited(status),
                Err(error) => SessionError::Io(error),
            },
            Err(error) => SessionError::Io(error),
        }
    }

    fn exited(&self, status: ExitStatus) -> SessionError {
        SessionError::HostExited {
            status,
            stderr: self.stderr_log(),
        }
    }

    fn force_stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn start_watch(&mut self) {
        let pid = self.child.os_pid();
        let slots = Arc::clone(&self.watch_slots);
        let stop = Arc::clone(&self.watch_stop);
        self.watch_stop.store(false, Ordering::Release);
        self.watch = Some(std::thread::spawn(move || watch_exit(pid, slots, stop)));
    }

    fn stop_watch(&mut self) {
        self.watch_stop.store(true, Ordering::Release);
        if let Some(handle) = self.watch.take() {
            let _ = handle.join();
        }
    }

    fn point_watch(&self) {
        if let Ok(mut slots) = self.watch_slots.lock() {
            slots.notice = self.region.notice_ptr();
            slots.dead = self.region.dead_ptr();
        }
    }

    fn install_region(&mut self, region: MappedRegion) {
        if let Ok(mut slots) = self.watch_slots.lock() {
            slots.notice = region.notice_ptr();
            slots.dead = region.dead_ptr();
            self.region = region;
        }
    }

    fn drain(&mut self) -> Result<(), SessionError> {
        loop {
            match self.control.try_recv()? {
                Some(message) => self.inbox.push_back(message),
                None => return Ok(()),
            }
        }
    }

    fn wait_message(
        &mut self,
        timeout: Duration,
        want: impl Fn(&ControlMessage) -> bool,
    ) -> Result<ControlMessage, SessionError> {
        let deadline = Instant::now() + timeout;
        loop {
            self.drain()?;
            if let Some(index) = self.inbox.iter().position(&want) {
                return self.inbox.remove(index).ok_or_else(|| {
                    SessionError::Protocol("queued control message disappeared".to_string())
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SessionError::Timeout);
            }
            match self
                .control
                .recv_timeout(remaining.min(Duration::from_millis(50)))
            {
                Ok(message) => self.inbox.push_back(message),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl HelperProcess {
    fn os_pid(&self) -> i32 {
        self.pid
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        let mut raw = 0;
        let rc = unsafe { libc::waitpid(self.pid, &mut raw, libc::WNOHANG) };
        if rc == 0 {
            return Ok(None);
        }
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let exit = ExitStatus::from_raw(raw);
        self.status = Some(exit);
        Ok(Some(exit))
    }

    /// Once reaped, the pid may belong to another process, so it is not signalled.
    fn kill(&mut self) -> std::io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        let mut raw = 0;
        loop {
            if unsafe { libc::waitpid(self.pid, &mut raw, 0) } >= 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        let exit = ExitStatus::from_raw(raw);
        self.status = Some(exit);
        Ok(exit)
    }
}

/// Consumes the workgroup send right whether or not the launch succeeds.
fn launch(
    host_bin: &Path,
    plugin: Option<&Path>,
    shm: i32,
    workgroup: Option<WorkgroupHandoff>,
    fault: Option<&str>,
) -> Result<Launched, SessionError> {
    let launched = launch_with(host_bin, plugin, shm, workgroup.as_ref(), fault);
    if let Some(handoff) = workgroup {
        crate::workgroup::deallocate_port(handoff.port);
    }
    launched
}

/// The parent is multithreaded, so the helper is started with `posix_spawn`
/// and never `fork`. A forked child runs libSystem's atfork handlers, and one
/// that finds an initializer another thread was inside dies with SIGKILL.
fn launch_with(
    host_bin: &Path,
    plugin: Option<&Path>,
    shm: i32,
    workgroup: Option<&WorkgroupHandoff>,
    fault: Option<&str>,
) -> Result<Launched, SessionError> {
    if workgroup.is_some_and(|handoff| handoff.period_ticks == 0) {
        return Err(SessionError::Workgroup(
            "workgroup period is zero".to_string(),
        ));
    }
    let (parent_socket, child_socket) = Socket::pair()?;
    let mut args = vec![
        OsString::from("--shm-fd"),
        OsString::from("3"),
        OsString::from("--control-fd"),
        OsString::from("4"),
    ];
    let name = workgroup.map(|handoff| {
        let name = crate::workgroup::service_name();
        args.push(OsString::from("--workgroup-name"));
        args.push(OsString::from(&name));
        args.push(OsString::from("--workgroup-ticks"));
        args.push(OsString::from(handoff.period_ticks.to_string()));
        name
    });
    if let Some(plugin) = plugin {
        args.push(OsString::from("--plugin"));
        args.push(plugin.as_os_str().to_os_string());
    }
    let fault_env;
    let extra: &[(&str, &str)] = if let Some(fault) = fault {
        fault_env = [("ECHODE_TEST_FAULT", fault)];
        &fault_env
    } else {
        &[]
    };
    let shm_dup = inheritable(shm)?;
    let control_dup = inheritable(child_socket.as_raw_fd())?;
    let spawned = spawn_helper(
        host_bin,
        &args,
        extra,
        &[(shm_dup.as_raw_fd(), 3), (control_dup.as_raw_fd(), 4)],
    )
    .map_err(|error| SessionError::Io(std::io::Error::other(error)))?;
    drop((shm_dup, control_dup, child_socket));
    let mut child = HelperProcess {
        pid: spawned.pid,
        status: None,
    };
    if let (Some(handoff), Some(name)) = (workgroup, name)
        && let Err(error) = crate::workgroup::send_workgroup_port(&name, handoff.port)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SessionError::Workgroup(error));
    }
    Ok(Launched {
        child,
        stderr: Box::new(spawned.stderr),
        control: parent_socket,
    })
}

/// A copy above the target numbers 3 and 4, so `dup2` in the spawn never lands
/// on a source it still needs. `dup2` clears CLOEXEC on the target in the child;
/// the copy keeps it here, so a spawn on another thread does not inherit it.
fn inheritable(fd: i32) -> std::io::Result<OwnedFd> {
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 5) };
    if dup < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: `fcntl` returned a new descriptor that nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(dup) })
    }
}

fn spawn_stderr_reader(mut pipe: Box<dyn Read + Send>, slot: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    if let Ok(mut text) = slot.lock() {
                        push_stderr(&mut text, &chunk);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
}

fn push_stderr(slot: &mut String, chunk: &str) {
    slot.push_str(chunk);
    if slot.len() > STDERR_CAP {
        let mut drop_n = slot.len() - STDERR_CAP;
        while drop_n < slot.len() && !slot.is_char_boundary(drop_n) {
            drop_n += 1;
        }
        slot.drain(..drop_n);
    }
}

fn watch_exit(pid: i32, slots: Arc<Mutex<WatchSlots>>, stop: Arc<AtomicBool>) {
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return;
    }
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut event = libc::kevent {
        ident: 0,
        filter: 0,
        flags: 0,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut registered = false;
    while !stop.load(Ordering::Acquire) {
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 200_000_000,
        };
        let changes = if registered { 0 } else { 1 };
        let ready = unsafe { libc::kevent(queue, &change, changes, &mut event, 1, &timeout) };
        registered = true;
        if ready > 0 && event.filter == libc::EVFILT_PROC && event.fflags & libc::NOTE_EXIT != 0 {
            if let Ok(guard) = slots.lock() {
                if !guard.dead.is_null() {
                    // SAFETY: `dead` addresses the mapping's flag while this guard is held.
                    unsafe { (*guard.dead).store(1, Ordering::Release) };
                }
                if !guard.notice.is_null() {
                    // SAFETY: `notice` addresses the mapping's wake word. The store
                    // happens before the wake, under the same guard that drops the mapping.
                    unsafe {
                        (*guard.notice).fetch_add(1, Ordering::Release);
                        wake_one(&*guard.notice, SyncFlags::PROCESS);
                    }
                }
            }
            break;
        }
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }
    }
    unsafe { libc::close(queue) };
}

#[cfg(test)]
mod tests {
    use super::push_stderr;

    #[test]
    fn stderr_keeps_the_tail() {
        let mut slot = String::new();
        push_stderr(&mut slot, &"a".repeat(70_000));
        assert!(slot.len() >= 64 * 1024);
        assert!(slot.len() <= 64 * 1024 + 3);
        assert!(slot.chars().all(|ch| ch == 'a'));
    }
}
