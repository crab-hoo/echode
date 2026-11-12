//! Pass a device Audio Workgroup into the helper and join it.
//!
//! The helper checks a bootstrap name in with `bootstrap_check_in` and receives
//! one Mach message. The parent looks that name up and sends the
//! `os_workgroup_copy_port` send right in the message. The helper rebuilds the
//! workgroup with `os_workgroup_create_with_port`, then joins after setting the
//! device period as a time-constraint policy.
//!
//! `posix_spawnattr_setspecialport_np` does not carry this port: a workgroup
//! send right makes `posix_spawn` return EINVAL, and the bootstrap slot arrives
//! in the child as a null port.

use crate::error::{SpawnError, SpawnStep, WorkgroupCall, WorkgroupError};
use crate::os_workgroup::{
    os_release, os_workgroup_copy_port, os_workgroup_create_with_port, os_workgroup_join,
    os_workgroup_join_token_opaque_s, os_workgroup_leave, os_workgroup_s,
};
use mach2::bootstrap::{bootstrap_check_in, bootstrap_look_up, bootstrap_port};
use mach2::mach_port::{mach_port_deallocate, mach_port_mod_refs};
use mach2::message::{
    MACH_MSG_TYPE_COPY_SEND, MACH_MSGH_BITS, MACH_MSGH_BITS_COMPLEX, MACH_RCV_MSG,
    MACH_RCV_TIMEOUT, MACH_SEND_MSG, MACH_SEND_TIMEOUT, mach_msg, mach_msg_body_t,
    mach_msg_header_t, mach_msg_port_descriptor_t,
};
use mach2::port::{MACH_PORT_NULL, MACH_PORT_RIGHT_RECEIVE, mach_port_t};
use std::ffi::{CString, OsString};
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

static NAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A retained OS workgroup. Keep this owner on the control thread; helpers
/// receive their own send rights and reconstruct their own retained objects.
pub struct RetainedWorkgroup {
    object: *mut os_workgroup_s,
    period_ticks: u32,
}

// SAFETY: workgroup retention and port copying are thread-safe OS operations.
// This owner never joins a thread or contains a thread-local membership token.
unsafe impl Send for RetainedWorkgroup {}
// SAFETY: as for `Send`: every operation this owner performs on the object is a thread-safe OS
// call.
unsafe impl Sync for RetainedWorkgroup {}

impl RetainedWorkgroup {
    /// Takes ownership of the caller's retained OS workgroup, even on error.
    ///
    /// # Safety
    /// `object` must be null or a retained os_workgroup_t owned by the caller.
    pub unsafe fn from_retained(
        object: *mut os_workgroup_s,
        period_ticks: u32,
    ) -> Result<Self, WorkgroupError> {
        let owner = Self {
            object,
            period_ticks,
        };
        if object.is_null() || period_ticks == 0 {
            return Err(WorkgroupError::InvalidDevice);
        }
        Ok(owner)
    }

    pub(crate) fn from_handoff(handoff: crate::WorkgroupHandoff) -> Result<Self, WorkgroupError> {
        if handoff.port == 0 || handoff.period_ticks == 0 {
            return Err(WorkgroupError::InvalidHandoff);
        }
        // SAFETY: handoff owns its send right until the object is reconstructed.
        let object =
            unsafe { os_workgroup_create_with_port(c"echode-parent".as_ptr(), handoff.port) };
        // SAFETY: the OS returns a retained object (or null on failure).
        unsafe { Self::from_retained(object, handoff.period_ticks) }
    }

    pub fn handoff(&self) -> Result<crate::WorkgroupHandoff, WorkgroupError> {
        Ok(crate::WorkgroupHandoff {
            // SAFETY: this owner keeps the workgroup retained for the call.
            port: unsafe { copy_workgroup_port(self.object) }?,
            period_ticks: self.period_ticks,
        })
    }
}

impl Drop for RetainedWorkgroup {
    fn drop(&mut self) {
        if !self.object.is_null() {
            // SAFETY: this owner consumes exactly one retained reference.
            unsafe { os_release(self.object.cast()) };
        }
    }
}

/// Joined membership. Drop it on the thread that joined.
pub struct WorkgroupMembership {
    workgroup: *mut os_workgroup_s,
    token: os_workgroup_join_token_opaque_s,
    joined: bool,
    release: bool,
}

impl Drop for WorkgroupMembership {
    fn drop(&mut self) {
        if self.joined {
            // SAFETY: this thread joined with this token, and the object is still retained.
            unsafe { os_workgroup_leave(self.workgroup, &mut self.token) };
        }
        if self.release && !self.workgroup.is_null() {
            // SAFETY: `os_workgroup_create_with_port` returned a retained object.
            unsafe { os_release(self.workgroup.cast()) };
        }
    }
}

/// Send right for `workgroup`. The caller deallocates it, or hands it to spawn.
///
/// # Safety
///
/// `workgroup` is a retained `os_workgroup_t` and stays valid for the call.
pub unsafe fn copy_workgroup_port(workgroup: *mut os_workgroup_s) -> Result<u32, WorkgroupError> {
    if workgroup.is_null() {
        return Err(WorkgroupError::NullWorkgroup);
    }
    let mut port = 0u32;
    // SAFETY: the workgroup object stays retained by the caller for this call.
    let status = unsafe { os_workgroup_copy_port(workgroup, &mut port) };
    if status != 0 || port == 0 {
        return Err(WorkgroupError::Call {
            call: WorkgroupCall::CopyPort,
            status,
        });
    }
    Ok(port)
}

pub(crate) fn deallocate_port(port: u32) {
    if port == 0 {
        return;
    }
    // SAFETY: `port` is a send right in this task. Deallocating it once drops that right.
    unsafe {
        mach_port_deallocate(this_task(), port);
    }
}

/// Drop the receive right `bootstrap_check_in` gave, which unregisters the name.
/// `mach_port_deallocate` refuses a receive right (KERN_INVALID_RIGHT), so it would stay.
fn release_receive_right(receive: mach_port_t) {
    if receive == MACH_PORT_NULL {
        return;
    }
    // SAFETY: `receive` is a receive right in this task. Removing its one reference destroys
    // the port; nothing else in this task uses the name afterwards.
    unsafe {
        mach_port_mod_refs(this_task(), receive, MACH_PORT_RIGHT_RECEIVE, -1);
    }
}

fn this_task() -> mach_port_t {
    // SAFETY: `mach_task_self` has no preconditions; it reads this task's own port.
    unsafe { mach2::traps::mach_task_self() }
}

/// Join `workgroup` on this thread. The caller keeps the object alive until drop.
///
/// # Safety
///
/// `workgroup` is a retained `os_workgroup_t` and outlives the membership.
pub unsafe fn join_workgroup(
    workgroup: *mut os_workgroup_s,
    period_ticks: u32,
) -> Result<WorkgroupMembership, WorkgroupError> {
    if workgroup.is_null() {
        return Err(WorkgroupError::NullWorkgroup);
    }
    prepare_thread(period_ticks)?;
    let mut membership = WorkgroupMembership {
        workgroup,
        token: os_workgroup_join_token_opaque_s::new(),
        joined: false,
        release: false,
    };
    // SAFETY: the caller retains `workgroup` until this membership is dropped.
    let joined = unsafe { os_workgroup_join(workgroup, &mut membership.token) };
    if joined != 0 {
        return Err(WorkgroupError::Call {
            call: WorkgroupCall::Join,
            status: joined,
        });
    }
    membership.joined = true;
    Ok(membership)
}

pub(crate) fn service_name() -> String {
    let n = NAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("dev.echode.workgroup.{}.{n}", std::process::id())
}

/// Send `port` to the helper that has checked `name` in. Does not consume `port`.
pub(crate) fn send_workgroup_port(name: &str, port: u32) -> Result<(), WorkgroupError> {
    if port == 0 {
        return Err(WorkgroupError::NullPort);
    }
    let c_name = CString::new(name).map_err(|_| WorkgroupError::NulInName)?;
    let remote = lookup_send_right(&c_name)?;
    let sent = send_port(remote, port);
    deallocate_port(remote);
    sent
}

/// Join the workgroup the parent sends to `name`.
pub fn join_passed_workgroup(
    name: &str,
    period_ticks: u32,
) -> Result<WorkgroupMembership, WorkgroupError> {
    let c_name = CString::new(name).map_err(|_| WorkgroupError::NulInName)?;
    let mut receive = 0u32;
    // SAFETY: `bootstrap_port` is this process's bootstrap port, and `c_name` is valid.
    let status = unsafe { bootstrap_check_in(bootstrap_port, c_name.as_ptr(), &mut receive) };
    if status != 0 || receive == 0 {
        return Err(WorkgroupError::CheckIn {
            status,
            port: receive,
        });
    }
    let received = receive_port(receive);
    release_receive_right(receive);
    let port = received?;
    if let Err(error) = prepare_thread(period_ticks) {
        deallocate_port(port);
        return Err(error);
    }
    // SAFETY: `port` is the send right just received, owned here until it is deallocated below, and
    // the name is a static C string.
    let workgroup = unsafe { os_workgroup_create_with_port(c"echode-helper".as_ptr(), port) };
    deallocate_port(port);
    if workgroup.is_null() {
        return Err(WorkgroupError::Create);
    }
    let mut membership = WorkgroupMembership {
        workgroup,
        token: os_workgroup_join_token_opaque_s::new(),
        joined: false,
        release: true,
    };
    // SAFETY: the membership owns the retained `workgroup` until it is dropped on this thread, and
    // the token lives in it.
    let joined = unsafe { os_workgroup_join(workgroup, &mut membership.token) };
    if joined != 0 {
        return Err(WorkgroupError::Call {
            call: WorkgroupCall::Join,
            status: joined,
        });
    }
    membership.joined = true;
    Ok(membership)
}

pub struct SpawnedHelper {
    pub pid: i32,
    pub stderr: File,
}

/// Spawn `bin` with stdin and stdout on `/dev/null` and stderr on a pipe.
/// The child receives only those and the `inherit` pairs; every other descriptor is closed.
pub(crate) fn spawn_helper(
    bin: &Path,
    args: &[OsString],
    extra_env: &[(&str, &str)],
    inherit: &[(i32, i32)],
) -> Result<SpawnedHelper, SpawnError> {
    let path = CString::new(bin.as_os_str().as_bytes()).map_err(|_| SpawnError::NulInPath)?;
    let mut argv_owned = Vec::with_capacity(args.len() + 1);
    argv_owned.push(path.clone());
    for arg in args {
        argv_owned.push(
            CString::new(arg.as_bytes()).map_err(|_| SpawnError::NulInArgument(arg.clone()))?,
        );
    }
    let mut argv: Vec<*mut std::ffi::c_char> = argv_owned
        .iter()
        .map(|arg| arg.as_ptr().cast_mut())
        .collect();
    argv.push(std::ptr::null_mut());

    let mut env_owned = Vec::new();
    for (key, value) in std::env::vars_os() {
        if extra_env.iter().any(|(name, _)| key == *name) {
            continue;
        }
        let mut pair = key;
        pair.push("=");
        pair.push(value);
        if let Ok(text) = CString::new(pair.as_bytes()) {
            env_owned.push(text);
        }
    }
    for (key, value) in extra_env {
        env_owned.push(
            CString::new(format!("{key}={value}")).map_err(|_| SpawnError::NulInEnvironment)?,
        );
    }
    let mut envp: Vec<*mut std::ffi::c_char> = env_owned
        .iter()
        .map(|item| item.as_ptr().cast_mut())
        .collect();
    envp.push(std::ptr::null_mut());

    let mut read_fd = -1;
    let mut write_fd = -1;
    // SAFETY: `argv` and `envp` are null-terminated arrays of pointers into `argv_owned` and
    // `env_owned`, which outlive the call.
    let spawned = unsafe { spawn_posix(&path, &argv, &envp, inherit, &mut read_fd, &mut write_fd) };
    if write_fd >= 0 {
        // SAFETY: `write_fd` is the pipe's write end, which `spawn_posix` opened and this function
        // owns.
        unsafe { libc::close(write_fd) };
    }
    let spawned = match spawned {
        Ok(pid) => pid,
        Err(error) => {
            if read_fd >= 0 {
                // SAFETY: `read_fd` is the pipe's read end, which `spawn_posix` opened and this
                // function owns.
                unsafe { libc::close(read_fd) };
            }
            return Err(error);
        }
    };
    if read_fd < 0 {
        // SAFETY: `spawned` is the child just started, which nothing has reaped yet, so its pid is
        // still its own.
        unsafe { libc::kill(spawned, libc::SIGKILL) };
        return Err(SpawnError::NoStderrPipe);
    }
    Ok(SpawnedHelper {
        pid: spawned,
        // SAFETY: `read_fd` is the read end of a pipe this function opened.
        stderr: unsafe { File::from_raw_fd(read_fd) },
    })
}

/// # Safety
/// `argv` and `envp` are null-terminated arrays of pointers to NUL-terminated
/// strings, and the strings outlive the call.
unsafe fn spawn_posix(
    path: &CString,
    argv: &[*mut std::ffi::c_char],
    envp: &[*mut std::ffi::c_char],
    inherit: &[(i32, i32)],
    read_fd: &mut i32,
    write_fd: &mut i32,
) -> Result<i32, SpawnError> {
    let mut pipes = [0i32; 2];
    // SAFETY: `pipes` has room for the two descriptors `pipe` writes.
    if unsafe { libc::pipe(pipes.as_mut_ptr()) } != 0 {
        return Err(SpawnError::Pipe(std::io::Error::last_os_error()));
    }
    // SAFETY: `pipes[0]` is an open descriptor this function owns; `fcntl` only duplicates it.
    let read = unsafe { libc::fcntl(pipes[0], libc::F_DUPFD_CLOEXEC, 3) };
    // SAFETY: `pipes[1]` is an open descriptor this function owns; `fcntl` only duplicates it.
    let write = unsafe { libc::fcntl(pipes[1], libc::F_DUPFD_CLOEXEC, 3) };
    // SAFETY: both descriptors are open and owned here, and their duplicates replace them.
    unsafe {
        libc::close(pipes[0]);
        libc::close(pipes[1]);
    }
    if read < 0 || write < 0 {
        if read >= 0 {
            // SAFETY: `read` is a duplicate this function owns and closes once.
            unsafe { libc::close(read) };
        }
        if write >= 0 {
            // SAFETY: `write` is a duplicate this function owns and closes once.
            unsafe { libc::close(write) };
        }
        return Err(SpawnError::DupPipe(std::io::Error::last_os_error()));
    }
    *read_fd = read;
    *write_fd = write;
    pipes = [read, write];

    let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
    let mut attr: libc::posix_spawnattr_t = std::ptr::null_mut();
    // SAFETY: `actions` is local storage that `posix_spawn_file_actions_init` initializes.
    let init_actions = unsafe { libc::posix_spawn_file_actions_init(&mut actions) };
    if init_actions != 0 {
        return Err(posix(SpawnStep::FileActions, init_actions));
    }
    // SAFETY: `attr` is local storage that `posix_spawnattr_init` initializes.
    let init_attr = unsafe { libc::posix_spawnattr_init(&mut attr) };
    if init_attr != 0 {
        // SAFETY: `actions` was initialized above and is destroyed once.
        unsafe { libc::posix_spawn_file_actions_destroy(&mut actions) };
        return Err(posix(SpawnStep::Attributes, init_attr));
    }

    let result =
        // SAFETY: `argv` and `envp` are as the caller promised, and `actions` and `attr` were
        // initialized above and outlive the call.
        unsafe { configure_and_spawn(path, argv, envp, pipes, inherit, &mut actions, &mut attr) };
    // SAFETY: `actions` and `attr` were initialized above and are destroyed once.
    unsafe {
        libc::posix_spawnattr_destroy(&mut attr);
        libc::posix_spawn_file_actions_destroy(&mut actions);
    }
    result
}

/// # Safety
/// As for [`spawn_posix`]. `actions` and `attr` were initialized, and the caller
/// destroys them after this returns.
unsafe fn configure_and_spawn(
    path: &CString,
    argv: &[*mut std::ffi::c_char],
    envp: &[*mut std::ffi::c_char],
    pipes: [i32; 2],
    inherit: &[(i32, i32)],
    actions: &mut libc::posix_spawn_file_actions_t,
    attr: &mut libc::posix_spawnattr_t,
) -> Result<i32, SpawnError> {
    // SAFETY: `attr` was initialized by the caller.
    let flags = unsafe {
        libc::posix_spawnattr_setflags(attr, libc::POSIX_SPAWN_CLOEXEC_DEFAULT as libc::c_short)
    };
    if flags != 0 {
        return Err(posix(SpawnStep::CloseOthers, flags));
    }
    let null = c"/dev/null".as_ptr();
    let open_in =
        // SAFETY: `actions` was initialized by the caller, and `null` is a static C string.
        unsafe { libc::posix_spawn_file_actions_addopen(actions, 0, null, libc::O_RDONLY, 0) };
    if open_in != 0 {
        return Err(posix(SpawnStep::Stdin, open_in));
    }
    let open_out =
        // SAFETY: `actions` was initialized by the caller, and `null` is a static C string.
        unsafe { libc::posix_spawn_file_actions_addopen(actions, 1, null, libc::O_WRONLY, 0) };
    if open_out != 0 {
        return Err(posix(SpawnStep::Stdout, open_out));
    }
    // SAFETY: `actions` was initialized by the caller.
    let dup_err = unsafe { libc::posix_spawn_file_actions_adddup2(actions, pipes[1], 2) };
    if dup_err != 0 {
        return Err(posix(SpawnStep::Stderr, dup_err));
    }
    // SAFETY: `actions` was initialized by the caller.
    let close_read = unsafe { libc::posix_spawn_file_actions_addclose(actions, pipes[0]) };
    if close_read != 0 {
        return Err(posix(SpawnStep::CloseStderrRead, close_read));
    }
    if pipes[1] != 2 {
        // SAFETY: `actions` was initialized by the caller.
        let close_write = unsafe { libc::posix_spawn_file_actions_addclose(actions, pipes[1]) };
        if close_write != 0 {
            return Err(posix(SpawnStep::CloseStderrWrite, close_write));
        }
    }
    for (src, dst) in inherit {
        // SAFETY: `actions` was initialized by the caller.
        let dup = unsafe { libc::posix_spawn_file_actions_adddup2(actions, *src, *dst) };
        if dup != 0 {
            return Err(posix(
                SpawnStep::Inherit {
                    from: *src,
                    to: *dst,
                },
                dup,
            ));
        }
        if src != dst {
            // SAFETY: `actions` was initialized by the caller.
            let close = unsafe { libc::posix_spawn_file_actions_addclose(actions, *src) };
            if close != 0 {
                return Err(posix(SpawnStep::CloseInherited(*src), close));
            }
        }
    }
    let mut pid = 0i32;
    // SAFETY: `path`, `argv` and `envp` are as the caller promised, `actions` and `attr` were
    // initialized by the caller, and `pid` is writable.
    let spawned = unsafe {
        libc::posix_spawn(
            &mut pid,
            path.as_ptr(),
            actions,
            attr,
            argv.as_ptr(),
            envp.as_ptr(),
        )
    };
    if spawned != 0 {
        return Err(posix(SpawnStep::Spawn, spawned));
    }
    Ok(pid)
}

fn posix(step: SpawnStep, code: i32) -> SpawnError {
    SpawnError::Posix { step, code }
}

fn lookup_send_right(name: &CString) -> Result<u32, WorkgroupError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last = 0i32;
    while std::time::Instant::now() < deadline {
        let mut port = 0u32;
        // SAFETY: `name` is a C string and `bootstrap_port` belongs to this process.
        let status = unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr(), &mut port) };
        if status == 0 && port != 0 {
            return Ok(port);
        }
        last = status;
        // `bootstrap_look_up` does not block. This control thread, at helper
        // start, polls for up to five seconds. The audio thread does not wait
        // here. Special ports did not deliver the send right (S3).
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Err(WorkgroupError::Call {
        call: WorkgroupCall::LookUp,
        status: last,
    })
}

/// A complex Mach message that carries one port.
#[repr(C)]
struct PortMessage {
    header: mach_msg_header_t,
    body: mach_msg_body_t,
    port: mach_msg_port_descriptor_t,
}

const SEND_SIZE: u32 = std::mem::size_of::<PortMessage>() as u32;

fn send_port(remote: mach_port_t, port: mach_port_t) -> Result<(), WorkgroupError> {
    let mut message = PortMessage {
        header: mach_msg_header_t {
            msgh_bits: MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0) | MACH_MSGH_BITS_COMPLEX,
            msgh_size: SEND_SIZE,
            msgh_remote_port: remote,
            msgh_local_port: MACH_PORT_NULL,
            msgh_voucher_port: MACH_PORT_NULL,
            msgh_id: 1,
        },
        body: mach_msg_body_t {
            msgh_descriptor_count: 1,
        },
        port: mach_msg_port_descriptor_t::new(port, MACH_MSG_TYPE_COPY_SEND),
    };
    // SAFETY: `message` is a complex Mach message of `SEND_SIZE` bytes. `remote` is a send right
    // and `port` is the workgroup send right. Both outlive the call. COPY_SEND keeps them.
    let status = unsafe {
        mach_msg(
            (&raw mut message).cast(),
            MACH_SEND_MSG | MACH_SEND_TIMEOUT,
            SEND_SIZE,
            0,
            MACH_PORT_NULL,
            5_000,
            MACH_PORT_NULL,
        )
    };
    if status != 0 {
        return Err(WorkgroupError::Call {
            call: WorkgroupCall::Send,
            status,
        });
    }
    Ok(())
}

fn receive_port(receive: mach_port_t) -> Result<mach_port_t, WorkgroupError> {
    let mut received = Received {
        message: PortMessage {
            header: mach_msg_header_t {
                msgh_size: std::mem::size_of::<Received>() as u32,
                ..mach_msg_header_t::default()
            },
            body: mach_msg_body_t::default(),
            port: mach_msg_port_descriptor_t::default(),
        },
        trailer: [0; 64],
    };
    // SAFETY: `receive` is the receive right from `bootstrap_check_in`. The
    // buffer is larger than the message so the trailer fits.
    let status = unsafe {
        mach_msg(
            (&raw mut received).cast(),
            MACH_RCV_MSG | MACH_RCV_TIMEOUT,
            0,
            received.message.header.msgh_size,
            receive,
            5_000,
            MACH_PORT_NULL,
        )
    };
    if status != 0 {
        return Err(WorkgroupError::Call {
            call: WorkgroupCall::Receive,
            status,
        });
    }
    let count = received.message.body.msgh_descriptor_count;
    let port = received.message.port.name;
    if count != 1 || port == 0 {
        return Err(WorkgroupError::Message { count, port });
    }
    Ok(port)
}

/// The message and room for the trailer the kernel appends.
#[repr(C)]
struct Received {
    message: PortMessage,
    trailer: [u8; 64],
}

fn prepare_thread(period_ticks: u32) -> Result<(), WorkgroupError> {
    // SAFETY: setting the calling thread's QoS has no pointer preconditions.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
    if !apply_time_constraint(period_ticks) {
        return Err(WorkgroupError::ThreadPolicy { period_ticks });
    }
    Ok(())
}

/// Set the calling thread's time-constraint policy for a period of `period_ticks` Mach ticks,
/// with half of it as computation. A workgroup join needs this first, or it returns EINVAL.
/// Returns false for a zero period or when the kernel refuses the policy.
pub fn apply_time_constraint(period_ticks: u32) -> bool {
    if period_ticks == 0 {
        return false;
    }
    let computation = (period_ticks / 2).max(1);
    let period = period_ticks.max(computation);
    let mut policy = libc::thread_time_constraint_policy {
        period,
        computation,
        constraint: period,
        preemptible: 1,
    };
    // SAFETY: `pthread_mach_thread_np` borrows this thread's port. The policy
    // lives on the stack for the duration of the call.
    let status = unsafe {
        libc::thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_TIME_CONSTRAINT_POLICY as u32,
            (&raw mut policy).cast::<libc::integer_t>(),
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    status == libc::KERN_SUCCESS
}

#[cfg(test)]
#[path = "workgroup_tests.rs"]
mod tests;
