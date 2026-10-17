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

use crate::sync;
use std::ffi::{CString, OsString};
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const QOS_USER_INTERACTIVE: u32 = 0x21;
static NAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A retained OS workgroup. Keep this owner on the control thread; helpers
/// receive their own send rights and reconstruct their own retained objects.
pub struct RetainedWorkgroup {
    object: *mut std::ffi::c_void,
    period_ticks: u32,
}

// SAFETY: workgroup retention and port copying are thread-safe OS operations.
// This owner never joins a thread or contains a thread-local membership token.
unsafe impl Send for RetainedWorkgroup {}
unsafe impl Sync for RetainedWorkgroup {}

impl RetainedWorkgroup {
    /// Takes ownership of the caller's retained OS workgroup, even on error.
    ///
    /// # Safety
    /// `object` must be null or a retained os_workgroup_t owned by the caller.
    pub unsafe fn from_retained(
        object: *mut std::ffi::c_void,
        period_ticks: u32,
    ) -> Result<Self, String> {
        let owner = Self {
            object,
            period_ticks,
        };
        if object.is_null() || period_ticks == 0 {
            return Err("device workgroup or period is invalid".into());
        }
        Ok(owner)
    }

    pub(crate) fn from_handoff(handoff: crate::WorkgroupHandoff) -> Result<Self, String> {
        if handoff.port == 0 || handoff.period_ticks == 0 {
            return Err("workgroup port or period is invalid".into());
        }
        // SAFETY: handoff owns its send right until the object is reconstructed.
        let object =
            unsafe { sync::os_workgroup_create_with_port(c"echode-parent".as_ptr(), handoff.port) };
        // SAFETY: the OS returns a retained object (or null on failure).
        unsafe { Self::from_retained(object, handoff.period_ticks) }
    }

    pub fn handoff(&self) -> Result<crate::WorkgroupHandoff, String> {
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
            unsafe { sync::os_release(self.object) };
        }
    }
}

#[repr(C)]
pub(crate) struct Token {
    sig: u32,
    opaque: [u8; 36],
}

/// Joined membership. Drop it on the thread that joined.
pub struct WorkgroupMembership {
    workgroup: *mut std::ffi::c_void,
    token: Token,
    joined: bool,
    release: bool,
}

impl Drop for WorkgroupMembership {
    fn drop(&mut self) {
        if self.joined {
            // SAFETY: this thread joined with this token, and the object is still retained.
            unsafe { sync::os_workgroup_leave(self.workgroup, &mut self.token) };
        }
        if self.release && !self.workgroup.is_null() {
            // SAFETY: `os_workgroup_create_with_port` returned a retained object.
            unsafe { sync::os_release(self.workgroup) };
        }
    }
}

/// Send right for `workgroup`. The caller deallocates it, or hands it to spawn.
///
/// # Safety
///
/// `workgroup` is a retained `os_workgroup_t` and stays valid for the call.
pub unsafe fn copy_workgroup_port(workgroup: *mut std::ffi::c_void) -> Result<u32, String> {
    if workgroup.is_null() {
        return Err("workgroup is null".to_string());
    }
    let mut port = 0u32;
    // SAFETY: the workgroup object stays retained by the caller for this call.
    let status = unsafe { sync::os_workgroup_copy_port(workgroup, &mut port) };
    if status != 0 || port == 0 {
        return Err(format!("os_workgroup_copy_port returned {status}"));
    }
    Ok(port)
}

pub(crate) fn deallocate_port(port: u32) {
    if port == 0 {
        return;
    }
    // SAFETY: `port` is a send right in this task. Deallocating it once drops that right.
    unsafe {
        sync::mach_port_deallocate(this_task(), port);
    }
}

fn this_task() -> u32 {
    // `mach2` 0.7 does not expose the current task port. libc still does, and
    // marks the wrapper deprecated.
    #[allow(deprecated)]
    unsafe {
        libc::mach_task_self()
    }
}

/// Join `workgroup` on this thread. The caller keeps the object alive until drop.
///
/// # Safety
///
/// `workgroup` is a retained `os_workgroup_t` and outlives the membership.
pub unsafe fn join_workgroup(
    workgroup: *mut std::ffi::c_void,
    period_ticks: u32,
) -> Result<WorkgroupMembership, String> {
    if workgroup.is_null() {
        return Err("workgroup is null".to_string());
    }
    prepare_thread(period_ticks)?;
    let mut membership = WorkgroupMembership {
        workgroup,
        token: Token {
            sig: 0,
            opaque: [0; 36],
        },
        joined: false,
        release: false,
    };
    // SAFETY: the caller retains `workgroup` until this membership is dropped.
    let joined = unsafe { sync::os_workgroup_join(workgroup, &mut membership.token) };
    if joined != 0 {
        return Err(format!("os_workgroup_join returned {joined}"));
    }
    membership.joined = true;
    Ok(membership)
}

pub(crate) fn service_name() -> String {
    let n = NAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("dev.echode.workgroup.{}.{n}", std::process::id())
}

/// Send `port` to the helper that has checked `name` in. Does not consume `port`.
pub(crate) fn send_workgroup_port(name: &str, port: u32) -> Result<(), String> {
    if port == 0 {
        return Err("workgroup port is null".to_string());
    }
    let c_name = CString::new(name).map_err(|_| "workgroup name contains a nul")?;
    let remote = lookup_send_right(&c_name)?;
    let sent = send_port(remote, port);
    deallocate_port(remote);
    sent
}

/// Join the workgroup the parent sends to `name`.
pub fn join_passed_workgroup(name: &str, period_ticks: u32) -> Result<WorkgroupMembership, String> {
    let c_name = CString::new(name).map_err(|_| "workgroup name contains a nul")?;
    let mut receive = 0u32;
    // SAFETY: `bootstrap_port` is this process's bootstrap port, and `c_name` is valid.
    let status =
        unsafe { sync::bootstrap_check_in(sync::bootstrap_port, c_name.as_ptr(), &mut receive) };
    if status != 0 || receive == 0 {
        return Err(format!(
            "bootstrap_check_in returned {status}, port {receive}"
        ));
    }
    let port = match receive_port(receive) {
        Ok(port) => port,
        Err(error) => {
            deallocate_port(receive);
            return Err(error);
        }
    };
    deallocate_port(receive);
    if let Err(error) = prepare_thread(period_ticks) {
        deallocate_port(port);
        return Err(error);
    }
    let workgroup = unsafe { sync::os_workgroup_create_with_port(c"echode-helper".as_ptr(), port) };
    deallocate_port(port);
    if workgroup.is_null() {
        return Err("os_workgroup_create_with_port returned null".to_string());
    }
    let mut membership = WorkgroupMembership {
        workgroup,
        token: Token {
            sig: 0,
            opaque: [0; 36],
        },
        joined: false,
        release: true,
    };
    let joined = unsafe { sync::os_workgroup_join(workgroup, &mut membership.token) };
    if joined != 0 {
        return Err(format!("os_workgroup_join returned {joined}"));
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
pub fn spawn_helper(
    bin: &Path,
    args: &[OsString],
    extra_env: &[(&str, &str)],
    inherit: &[(i32, i32)],
) -> Result<SpawnedHelper, String> {
    let path = CString::new(bin.as_os_str().as_bytes()).map_err(|_| "host path contains a nul")?;
    let mut argv_owned = Vec::with_capacity(args.len() + 1);
    argv_owned.push(path.clone());
    for arg in args {
        argv_owned.push(
            CString::new(arg.as_bytes())
                .map_err(|_| format!("argument contains a nul: {arg:?}"))?,
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
            CString::new(format!("{key}={value}")).map_err(|_| "environment contains a nul")?,
        );
    }
    let mut envp: Vec<*mut std::ffi::c_char> = env_owned
        .iter()
        .map(|item| item.as_ptr().cast_mut())
        .collect();
    envp.push(std::ptr::null_mut());

    let mut read_fd = -1;
    let mut write_fd = -1;
    let spawned = unsafe { spawn_posix(&path, &argv, &envp, inherit, &mut read_fd, &mut write_fd) };
    if write_fd >= 0 {
        unsafe { libc::close(write_fd) };
    }
    let spawned = match spawned {
        Ok(pid) => pid,
        Err(error) => {
            if read_fd >= 0 {
                unsafe { libc::close(read_fd) };
            }
            return Err(error);
        }
    };
    if read_fd < 0 {
        unsafe { libc::kill(spawned, libc::SIGKILL) };
        return Err("stderr pipe was not created".to_string());
    }
    Ok(SpawnedHelper {
        pid: spawned,
        // SAFETY: `read_fd` is the read end of a pipe this function opened.
        stderr: unsafe { File::from_raw_fd(read_fd) },
    })
}

unsafe fn spawn_posix(
    path: &CString,
    argv: &[*mut std::ffi::c_char],
    envp: &[*mut std::ffi::c_char],
    inherit: &[(i32, i32)],
    read_fd: &mut i32,
    write_fd: &mut i32,
) -> Result<i32, String> {
    let mut pipes = [0i32; 2];
    if unsafe { libc::pipe(pipes.as_mut_ptr()) } != 0 {
        return Err(format!("pipe: {}", std::io::Error::last_os_error()));
    }
    let read = unsafe { libc::fcntl(pipes[0], libc::F_DUPFD_CLOEXEC, 3) };
    let write = unsafe { libc::fcntl(pipes[1], libc::F_DUPFD_CLOEXEC, 3) };
    unsafe {
        libc::close(pipes[0]);
        libc::close(pipes[1]);
    }
    if read < 0 || write < 0 {
        if read >= 0 {
            unsafe { libc::close(read) };
        }
        if write >= 0 {
            unsafe { libc::close(write) };
        }
        return Err(format!("dup pipe: {}", std::io::Error::last_os_error()));
    }
    *read_fd = read;
    *write_fd = write;
    pipes = [read, write];

    let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
    let mut attr: libc::posix_spawnattr_t = std::ptr::null_mut();
    let init_actions = unsafe { libc::posix_spawn_file_actions_init(&mut actions) };
    if init_actions != 0 {
        return Err(format!("posix_spawn_file_actions_init: {init_actions}"));
    }
    let init_attr = unsafe { libc::posix_spawnattr_init(&mut attr) };
    if init_attr != 0 {
        unsafe { libc::posix_spawn_file_actions_destroy(&mut actions) };
        return Err(format!("posix_spawnattr_init: {init_attr}"));
    }

    let result =
        unsafe { configure_and_spawn(path, argv, envp, pipes, inherit, &mut actions, &mut attr) };
    unsafe {
        libc::posix_spawnattr_destroy(&mut attr);
        libc::posix_spawn_file_actions_destroy(&mut actions);
    }
    result
}

unsafe fn configure_and_spawn(
    path: &CString,
    argv: &[*mut std::ffi::c_char],
    envp: &[*mut std::ffi::c_char],
    pipes: [i32; 2],
    inherit: &[(i32, i32)],
    actions: &mut libc::posix_spawn_file_actions_t,
    attr: &mut libc::posix_spawnattr_t,
) -> Result<i32, String> {
    let flags = unsafe {
        libc::posix_spawnattr_setflags(attr, libc::POSIX_SPAWN_CLOEXEC_DEFAULT as libc::c_short)
    };
    if flags != 0 {
        return Err(format!("close other descriptors: {flags}"));
    }
    let null = c"/dev/null".as_ptr();
    let open_in =
        unsafe { libc::posix_spawn_file_actions_addopen(actions, 0, null, libc::O_RDONLY, 0) };
    if open_in != 0 {
        return Err(format!("redirect stdin: {open_in}"));
    }
    let open_out =
        unsafe { libc::posix_spawn_file_actions_addopen(actions, 1, null, libc::O_WRONLY, 0) };
    if open_out != 0 {
        return Err(format!("redirect stdout: {open_out}"));
    }
    let dup_err = unsafe { libc::posix_spawn_file_actions_adddup2(actions, pipes[1], 2) };
    if dup_err != 0 {
        return Err(format!("redirect stderr: {dup_err}"));
    }
    let close_read = unsafe { libc::posix_spawn_file_actions_addclose(actions, pipes[0]) };
    if close_read != 0 {
        return Err(format!("close stderr read end: {close_read}"));
    }
    if pipes[1] != 2 {
        let close_write = unsafe { libc::posix_spawn_file_actions_addclose(actions, pipes[1]) };
        if close_write != 0 {
            return Err(format!("close stderr write end: {close_write}"));
        }
    }
    for (src, dst) in inherit {
        let dup = unsafe { libc::posix_spawn_file_actions_adddup2(actions, *src, *dst) };
        if dup != 0 {
            return Err(format!("inherit fd {src} -> {dst}: {dup}"));
        }
        if src != dst {
            let close = unsafe { libc::posix_spawn_file_actions_addclose(actions, *src) };
            if close != 0 {
                return Err(format!("close inherited fd {src}: {close}"));
            }
        }
    }
    let mut pid = 0i32;
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
        return Err(format!("posix_spawn: {spawned}"));
    }
    Ok(pid)
}

fn lookup_send_right(name: &CString) -> Result<u32, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last = 0i32;
    while std::time::Instant::now() < deadline {
        let mut port = 0u32;
        // SAFETY: `name` is a C string and `bootstrap_port` belongs to this process.
        let status =
            unsafe { sync::bootstrap_look_up(sync::bootstrap_port, name.as_ptr(), &mut port) };
        if status == 0 && port != 0 {
            return Ok(port);
        }
        last = status;
        // `bootstrap_look_up` does not block. This control thread, at helper
        // start, polls for up to five seconds. The audio thread does not wait
        // here. Special ports did not deliver the send right (S3).
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Err(format!("bootstrap_look_up returned {last}"))
}

#[repr(C)]
struct PortMessage {
    bits: u32,
    size: u32,
    remote: u32,
    local: u32,
    voucher: u32,
    id: u32,
    count: u32,
    name: u32,
    pad: u32,
    kind: u32,
}

const _: () = assert!(std::mem::size_of::<PortMessage>() == 40);

const MACH_SEND_MSG: u32 = 0x1;
const MACH_RCV_MSG: u32 = 0x2;
const MACH_SEND_TIMEOUT: u32 = 0x10;
const MACH_RCV_TIMEOUT: u32 = 0x100;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const SEND_SIZE: u32 = 40;

fn send_port(remote: u32, port: u32) -> Result<(), String> {
    let mut message = PortMessage {
        bits: MACH_MSG_TYPE_COPY_SEND | MACH_MSGH_BITS_COMPLEX,
        size: SEND_SIZE,
        remote,
        local: 0,
        voucher: 0,
        id: 1,
        count: 1,
        name: port,
        pad: 0,
        // disposition in bits 16..24, descriptor type 0 in bits 24..32.
        kind: MACH_MSG_TYPE_COPY_SEND << 16,
    };
    // SAFETY: `message` is a complex Mach message. `remote` is a send right and
    // `port` is the workgroup send right. Both outlive the call. COPY_SEND keeps them.
    let status = unsafe {
        sync::mach_msg(
            (&raw mut message).cast(),
            MACH_SEND_MSG | MACH_SEND_TIMEOUT,
            SEND_SIZE,
            0,
            0,
            5_000,
            0,
        )
    };
    if status != 0 {
        return Err(format!("mach_msg send returned {status:#x}"));
    }
    Ok(())
}

fn receive_port(receive: u32) -> Result<u32, String> {
    let mut received = Received {
        message: PortMessage {
            bits: 0,
            size: std::mem::size_of::<Received>() as u32,
            remote: 0,
            local: 0,
            voucher: 0,
            id: 0,
            count: 0,
            name: 0,
            pad: 0,
            kind: 0,
        },
        trailer: [0; 64],
    };
    // SAFETY: `receive` is the receive right from `bootstrap_check_in`. The
    // buffer is larger than the message so the trailer fits.
    let status = unsafe {
        sync::mach_msg(
            (&raw mut received).cast(),
            MACH_RCV_MSG | MACH_RCV_TIMEOUT,
            0,
            received.message.size,
            receive,
            5_000,
            0,
        )
    };
    if status != 0 {
        return Err(format!("mach_msg receive returned {status:#x}"));
    }
    if received.message.count != 1 || received.message.name == 0 {
        return Err(format!(
            "workgroup message had count {} and port {}",
            received.message.count, received.message.name
        ));
    }
    Ok(received.message.name)
}

#[repr(C)]
struct Received {
    message: PortMessage,
    trailer: [u8; 64],
}

fn prepare_thread(period_ticks: u32) -> Result<(), String> {
    // libc 0.2.190 does not re-export this call. 0x21 is QOS_CLASS_USER_INTERACTIVE.
    unsafe {
        sync::pthread_set_qos_class_self_np(QOS_USER_INTERACTIVE, 0);
    }
    if !apply_time_constraint(period_ticks) {
        return Err(format!(
            "thread_policy_set failed for a period of {period_ticks} ticks"
        ));
    }
    Ok(())
}

fn apply_time_constraint(period_ticks: u32) -> bool {
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
