//! FSEvents watch for plugin folders.
//!
//! Any change wakes the caller. The caller lists bundles again and hands them
//! to [`crate::PluginCatalog`], which waits until the writes go quiet.

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const FILE_EVENTS: u32 = 0x0000_0010;
const NO_DEFER: u32 = 0x0000_0002;
const WATCH_ROOT: u32 = 0x0000_0004;
const SINCE_NOW: u64 = u64::MAX;

struct CallbackInfo {
    tx: std::sync::mpsc::Sender<()>,
}

#[repr(C)]
struct StreamContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    copy_description: *const c_void,
}

struct RunLoop(*mut c_void);

// `CFRunLoopStop` is documented to be callable from another thread.
unsafe impl Send for RunLoop {}
unsafe impl Sync for RunLoop {}

/// Background watch. Dropping it stops the FSEvents stream.
pub struct FolderWatch {
    runloop: Arc<Mutex<Option<RunLoop>>>,
    thread: Option<JoinHandle<()>>,
    rx: Receiver<()>,
}

impl FolderWatch {
    pub fn start(root: &Path) -> Result<Self, String> {
        let root = root.to_path_buf();
        let (tx, rx) = mpsc::channel();
        let runloop = Arc::new(Mutex::new(None));
        let published = Arc::clone(&runloop);
        let thread = thread::Builder::new()
            .name("echode-plugin-watch".to_string())
            .spawn(move || watch_thread(root, tx, published))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            runloop,
            thread: Some(thread),
            rx,
        })
    }

    /// Waits until the folder changes or `timeout` elapses.
    pub fn wait(&self, timeout: Duration) -> bool {
        match self.rx.recv_timeout(timeout) {
            Ok(()) => {
                while self.rx.try_recv().is_ok() {}
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => false,
        }
    }
}

impl Drop for FolderWatch {
    fn drop(&mut self) {
        if let Some(runloop) = self
            .runloop
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|loop_| loop_.0))
        {
            unsafe { CFRunLoopStop(runloop) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn watch_thread(
    root: PathBuf,
    tx: std::sync::mpsc::Sender<()>,
    runloop: Arc<Mutex<Option<RunLoop>>>,
) {
    let mut c_path = root.into_os_string().into_encoded_bytes();
    c_path.push(0);
    let cf_path =
        unsafe { CFStringCreateWithFileSystemRepresentation(ptr::null(), c_path.as_ptr().cast()) };
    if cf_path.is_null() {
        return;
    }
    let values = [cf_path];
    let array = unsafe { CFArrayCreate(ptr::null(), values.as_ptr(), 1, &kCFTypeArrayCallBacks) };
    if array.is_null() {
        unsafe { CFRelease(cf_path) };
        return;
    }
    let info = Box::into_raw(Box::new(CallbackInfo { tx }));
    let mut context = StreamContext {
        version: 0,
        info: info.cast(),
        retain: ptr::null(),
        release: Some(release_info),
        copy_description: ptr::null(),
    };
    let stream = unsafe {
        FSEventStreamCreate(
            ptr::null(),
            stream_callback,
            &mut context,
            array,
            SINCE_NOW,
            0.1,
            FILE_EVENTS | NO_DEFER | WATCH_ROOT,
        )
    };
    unsafe { CFRelease(array) };
    if stream.is_null() {
        unsafe { drop(Box::from_raw(info)) };
        return;
    }
    unsafe {
        let loop_ = CFRunLoopGetCurrent();
        FSEventStreamScheduleWithRunLoop(stream, loop_, kCFRunLoopDefaultMode);
        if !FSEventStreamStart(stream) {
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
            return;
        }
        if let Ok(mut slot) = runloop.lock() {
            *slot = Some(RunLoop(loop_));
        }
        CFRunLoopRun();
        FSEventStreamStop(stream);
        FSEventStreamInvalidate(stream);
        FSEventStreamRelease(stream);
    }
}

unsafe extern "C" fn release_info(info: *const c_void) {
    if !info.is_null() {
        drop(unsafe { Box::from_raw(info as *mut CallbackInfo) });
    }
}

unsafe extern "C" fn stream_callback(
    _stream: *mut c_void,
    info: *mut c_void,
    _num_events: usize,
    _paths: *mut c_void,
    _flags: *const u32,
    _ids: *const u64,
) {
    if info.is_null() {
        return;
    }
    let context = unsafe { &*(info as *const CallbackInfo) };
    let _ = context.tx.send(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{PluginCatalog, list_bundles};
    use std::time::Instant;

    #[test]
    fn creating_and_removing_a_bundle_shows_up_in_the_catalog() {
        let root = std::env::temp_dir().join(format!(
            "echode-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let watch = FolderWatch::start(&root).expect("watch");
        // The stream may already be running. Give it a moment to arm.
        std::thread::sleep(Duration::from_millis(200));

        let bundle = root.join("Saw.clap");
        std::fs::create_dir(&bundle).unwrap();
        assert!(
            watch.wait(Duration::from_secs(5)),
            "FSEvents did not report the new bundle"
        );
        let mut catalog = PluginCatalog::new(Duration::ZERO);
        let now = Instant::now();
        catalog.observe(now, &list_bundles(&root));
        assert!(catalog.contains(&bundle));
        assert_eq!(catalog.due(now), vec![bundle.clone()]);

        std::fs::remove_dir(&bundle).unwrap();
        assert!(
            watch.wait(Duration::from_secs(5)),
            "FSEvents did not report the removed bundle"
        );
        catalog.observe(Instant::now(), &list_bundles(&root));
        assert!(!catalog.contains(&bundle));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithFileSystemRepresentation(
        allocator: *const c_void,
        path: *const c_char,
    ) -> *const c_void;
    fn CFArrayCreate(
        allocator: *const c_void,
        values: *const *const c_void,
        count: isize,
        callbacks: *const c_void,
    ) -> *const c_void;
    fn CFRelease(value: *const c_void);
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopRun();
    fn CFRunLoopStop(runloop: *mut c_void);
    static kCFTypeArrayCallBacks: c_void;
    static kCFRunLoopDefaultMode: *const c_void;
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            usize,
            *mut c_void,
            *const u32,
            *const u64,
        ),
        context: *mut StreamContext,
        paths: *const c_void,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> *mut c_void;
    fn FSEventStreamScheduleWithRunLoop(
        stream: *mut c_void,
        runloop: *mut c_void,
        mode: *const c_void,
    );
    fn FSEventStreamStart(stream: *mut c_void) -> bool;
    fn FSEventStreamStop(stream: *mut c_void);
    fn FSEventStreamInvalidate(stream: *mut c_void);
    fn FSEventStreamRelease(stream: *mut c_void);
}
