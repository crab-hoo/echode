//! Room for one descriptor set per helper.
//!
//! An app started from the Finder inherits launchd's soft limit of 256 open
//! descriptors. The parent keeps four for each helper (shared memory, control
//! socket, stderr pipe, exit watch) and briefly more while one starts, so the
//! default would stop helpers after a few dozen plugins. The plugin count has
//! no fixed limit, so the first helper raises the soft limit (ADR 0237).

use std::sync::Once;

/// `<sys/syslimits.h>`. setrlimit(2) refuses `RLIM_INFINITY` for descriptors
/// and names `min(OPEN_MAX, rlim_max)` as the value it always accepts.
const OPEN_MAX: libc::rlim_t = 10_240;

/// Raises the soft descriptor limit once per process. The hard limit is left
/// alone. A lower result only means helpers may fail to start later, where the
/// error names the cause, so a refusal here is not an error.
pub(crate) fn raise_descriptor_limit() {
    static RAISED: Once = Once::new();
    RAISED.call_once(|| {
        let Some(limit) = descriptor_limit() else {
            return;
        };
        // The per-process ceiling first, then the value setrlimit(2) documents.
        for ceiling in [max_files_per_process(), Some(OPEN_MAX)]
            .into_iter()
            .flatten()
        {
            let target = limit.rlim_max.min(ceiling);
            if target <= limit.rlim_cur {
                return;
            }
            let raised = libc::rlimit {
                rlim_cur: target,
                rlim_max: limit.rlim_max,
            };
            // SAFETY: `raised` is a valid rlimit that keeps the hard limit.
            if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
                return;
            }
        }
    });
}

fn descriptor_limit() -> Option<libc::rlimit> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, writable rlimit for the call's duration.
    (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0).then_some(limit)
}

fn max_files_per_process() -> Option<libc::rlim_t> {
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is NUL-terminated, and `value` and `size` describe one
    // writable int. No new value is written.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && value > 0).then_some(value as libc::rlim_t)
}
