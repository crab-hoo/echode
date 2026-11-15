use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// A host whose child was already reaped externally. No OS wait failure or
/// shutdown exchange needs to be simulated, and no live PID can be signalled.
fn reaped_host() -> PluginHost {
    let mut child = Command::new("/usr/bin/true").spawn().unwrap();
    let status = child.wait().unwrap();
    let region = Arc::new(MappedRegion::create_shared(SessionLayout::stereo()).unwrap());
    let (parent, _helper) = Socket::pair().unwrap();
    PluginHost {
        workgroup: None,
        root: Arc::clone(&region),
        previous_region: None,
        watch_slots: Arc::new(Mutex::new(WatchSlots {
            notice: region.notice_ptr(),
            dead: region.dead_ptr(),
            fan: ptr::null(),
        })),
        region,
        child: HelperProcess {
            pid: child.id() as i32,
            status: Some(status),
        },
        control: PluginControl::new(parent),
        stderr: Default::default(),
        closed: false,
        reset_pending: false,
        host_bin: PathBuf::new(),
        plugin: None,
        plugin_id: None,
        sample_rate: 0,
        watch: None,
        watch_stop: Arc::new(AtomicBool::new(false)),
        fan: None,
        ports: PortLayout::default(),
    }
}

#[test]
fn audit_dia08_drop_joins_the_watcher_even_when_shutdown_cannot_wait() {
    for explicit_shutdown in [false, true] {
        let mut host = reaped_host();
        // Discard only the cached status, making the real waitpid return ECHILD.
        host.child.status = None;
        let stop = Arc::clone(&host.watch_stop);
        let watched_stop = Arc::clone(&stop);
        let (finished, done) = std::sync::mpsc::channel();
        // Keep the watcher alive until stop_watch runs. It deliberately does
        // not dereference the mapping, so the broken Drop is safe to test.
        host.watch = Some(std::thread::spawn(move || {
            while !watched_stop.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            finished.send(()).unwrap();
        }));
        if explicit_shutdown {
            assert!(matches!(host.shutdown(), Err(SessionError::Io(error))
                if error.raw_os_error() == Some(libc::ECHILD)));
        }
        drop(host);
        let stopped_before_cleanup = stop.load(Ordering::Acquire);
        let joined_before_cleanup = done.try_recv().is_ok();
        // Also release the old, detached watcher when testing the broken Drop.
        stop.store(true, Ordering::Release);
        if !joined_before_cleanup {
            done.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(
            stopped_before_cleanup && joined_before_cleanup,
            "DIA-08: Drop must stop and join the watcher before releasing mappings"
        );
    }
}

#[test]
fn audit_dia08_a_failed_restart_reaps_its_unready_helper_before_returning() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "echode-restart-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let binary = directory.join("unready-helper");
    // A real child keeps its inherited descriptors but never marks the mapping
    // ready. exec leaves no descendant process when the host kills the child.
    std::fs::write(&binary, "#!/bin/sh\nexec /bin/sleep 60\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut host = reaped_host();
    host.host_bin = binary;
    let failed = host.restart();
    let reaped = host.child.status.is_some();
    let joined = host.watch.is_none();
    drop(host);
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(matches!(failed, Err(SessionError::Timeout)), "{failed:?}");
    assert!(
        reaped && joined,
        "DIA-08: a failed restart must reap its new helper and join its watcher"
    );
}
