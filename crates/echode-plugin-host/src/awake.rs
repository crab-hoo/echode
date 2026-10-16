//! Keeps a helper from App Nap while it serves audio. Lid close is not blocked.
//!
//! `NSActivityOptions::UserInteractive` is already `UserInitiated | LatencyCritical`
//! in objc2-foundation. Passing `LatencyCritical` again would not add a flag.

use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

/// `endActivity:` is safe from any thread. objc2 does not mark the token `Send`.
struct End(Option<Box<dyn FnOnce()>>);
// SAFETY: the closure only calls `endActivity:` and releases the token. Both
// are thread-safe, and the token is not shared.
unsafe impl Send for End {}

pub struct HelperActivity {
    end: End,
}

impl HelperActivity {
    pub fn begin() -> Self {
        let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
            NSActivityOptions::UserInteractive,
            &NSString::from_str("Echode plugin helper"),
        );
        Self {
            end: End(Some(Box::new(move || {
                // SAFETY: the token came from beginActivityWithOptions:reason: on this process.
                unsafe { NSProcessInfo::processInfo().endActivity(&token) }
            }))),
        }
    }
}

impl Drop for HelperActivity {
    fn drop(&mut self) {
        if let Some(end) = self.end.0.take() {
            end();
        }
    }
}
