//! Holds a public `NSProcessInfo` activity while playback or export is running.
//!
//! `UserInteractive` is `UserInitiated | LatencyCritical`. It asks the system
//! to skip App Nap and idle sleep. Closing the lid still sleeps the machine.
//! The token is released on drop.

use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

/// `endActivity:` is safe from any thread. objc2 does not mark the token `Send`
/// because `NSObjectProtocol` is not, so the wrapper carries that fact.
struct End(Option<Box<dyn FnOnce()>>);
// SAFETY: the closure only calls `endActivity:` and releases the token. Both
// are thread-safe, and the token is not shared.
unsafe impl Send for End {}

pub struct UserActivity {
    end: End,
}

impl UserActivity {
    pub fn begin(reason: &str) -> Self {
        let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
            NSActivityOptions::UserInteractive,
            &NSString::from_str(reason),
        );
        Self {
            end: End(Some(Box::new(move || {
                // SAFETY: the token came from beginActivityWithOptions:reason: on this process.
                unsafe { NSProcessInfo::processInfo().endActivity(&token) }
            }))),
        }
    }
}

impl Drop for UserActivity {
    fn drop(&mut self) {
        if let Some(end) = self.end.0.take() {
            end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UserActivity;

    #[test]
    fn an_activity_can_begin_and_end() {
        drop(UserActivity::begin("Echode test"));
    }
}
