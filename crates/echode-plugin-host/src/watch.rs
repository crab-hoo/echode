//! FSEvents watch for plugin folders.
//!
//! Any change wakes the caller. The caller lists bundles again and hands them
//! to [`crate::PluginCatalog`], which waits until the writes go quiet.
//!
//! The stream is scheduled on a dispatch queue (`FSEventStreamSetDispatchQueue`).
//! Stopping it does not wait for a run loop to start.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2_core_foundation::{CFArray, CFRetained, CFString};
use objc2_core_services::{
    ConstFSEventStreamRef, FSEventStreamContext, FSEventStreamCreate, FSEventStreamEventFlags,
    FSEventStreamEventId, FSEventStreamInvalidate, FSEventStreamRef, FSEventStreamRelease,
    FSEventStreamSetDispatchQueue, FSEventStreamStart, FSEventStreamStop,
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagNoDefer,
    kFSEventStreamCreateFlagWatchRoot, kFSEventStreamEventIdSinceNow,
};

struct CallbackInfo {
    tx: std::sync::mpsc::Sender<()>,
}

/// Background watch. Dropping it stops the FSEvents stream.
pub struct FolderWatch {
    stream: FSEventStreamRef,
    /// Kept until after the stream is invalidated. The queue must still be
    /// scheduled when `FSEventStreamInvalidate` runs.
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
    rx: Receiver<()>,
}

impl FolderWatch {
    pub fn start(root: &Path) -> Result<Self, String> {
        let Some(path) = root.to_str() else {
            return Err("plugin folder path is not utf-8".to_string());
        };
        let cf_path = CFString::from_str(path);
        let paths = CFArray::from_objects(&[&*cf_path]);
        // SAFETY: `CFArray<CFString>` is the same object as an untyped `CFArray`.
        // The stream retains that array and does not use the element type.
        let paths: CFRetained<CFArray> = unsafe { CFRetained::cast_unchecked(paths) };
        let (tx, rx) = mpsc::channel();
        let info = Box::into_raw(Box::new(CallbackInfo { tx }));
        let mut context = FSEventStreamContext {
            version: 0,
            info: info.cast(),
            retain: None,
            release: Some(release_info),
            copyDescription: None,
        };
        let stream = unsafe {
            FSEventStreamCreate(
                None,
                Some(stream_callback),
                &mut context,
                &paths,
                kFSEventStreamEventIdSinceNow,
                0.1,
                kFSEventStreamCreateFlagFileEvents
                    | kFSEventStreamCreateFlagNoDefer
                    | kFSEventStreamCreateFlagWatchRoot,
            )
        };
        if stream.is_null() {
            drop(unsafe { Box::from_raw(info) });
            return Err("FSEventStreamCreate failed".to_string());
        }
        let queue = DispatchQueue::new("dev.echode.plugin-watch", None);
        let started = unsafe {
            FSEventStreamSetDispatchQueue(stream, Some(&*queue));
            FSEventStreamStart(stream)
        };
        if !started {
            // The stream was never started, so `FSEventStreamStop` is not
            // allowed. Invalidate still releases the callback info.
            unsafe {
                FSEventStreamInvalidate(stream);
                FSEventStreamRelease(stream);
            }
            return Err("FSEventStreamStart failed".to_string());
        }
        Ok(Self {
            stream,
            _queue: queue,
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
        // `FSEventStreamStop` returns only after the callback will not run again.
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
        }
    }
}

/// Frees the info pointer when the stream is deallocated.
///
/// `FSEventStreamCreate` does not call `retain` when that callback is null,
/// and it does call `release` on deallocation (FSEvents.h).
unsafe extern "C-unwind" fn release_info(info: *const c_void) {
    if !info.is_null() {
        drop(unsafe { Box::from_raw(info as *mut CallbackInfo) });
    }
}

unsafe extern "C-unwind" fn stream_callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    _num_events: usize,
    _paths: NonNull<c_void>,
    _flags: NonNull<FSEventStreamEventFlags>,
    _ids: NonNull<FSEventStreamEventId>,
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
        // The stream is started before `start` returns. Give FSEvents a moment
        // to arm before the first change.
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
