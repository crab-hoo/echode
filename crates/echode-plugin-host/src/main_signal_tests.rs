use super::*;

#[test]
fn realtime_notifications_are_deferred_and_coalesced_under_rtsan() {
    let signal = MainSignal::default();
    // No run loop is installed: even an early request must survive until control relays it.
    for _ in 0..3 {
        assert!(signal.signal(true), "realtime notification must be relayed");
    }
    assert!(signal.pending.load(Ordering::Acquire));
    signal.relay();
    assert!(!signal.pending.load(Ordering::Acquire));
    assert!(!signal.signal(false), "ordinary notifications are direct");
    assert!(!signal.pending.load(Ordering::Acquire));
    assert!(signal.signal(true));
    assert!(signal.pending.load(Ordering::Acquire));
    signal.relay();
    assert!(!signal.pending.load(Ordering::Acquire));
}

#[test]
fn deferred_notifications_reach_the_source_and_can_request_another_callback() {
    use objc2_core_foundation::{CFRunLoopSourceContext, kCFRunLoopDefaultMode};
    use std::ffi::c_void;
    use std::sync::atomic::AtomicUsize;

    struct Context {
        signal: MainSignal,
        calls: AtomicUsize,
    }
    struct Registration<'a> {
        source: &'a CFRunLoopSource,
        run_loop: &'a CFRunLoop,
    }

    impl Drop for Registration<'_> {
        fn drop(&mut self) {
            // SAFETY: the mode is a process-wide constant. Unregister even on a test panic.
            self.run_loop
                .remove_source(Some(self.source), unsafe { kCFRunLoopDefaultMode });
        }
    }

    unsafe extern "C-unwind" fn perform(info: *mut c_void) {
        // SAFETY: the test retains `context` until the source is removed.
        let context = unsafe { &*info.cast::<Context>() };
        if context.calls.fetch_add(1, Ordering::AcqRel) == 0 {
            // Model a realtime request arriving while main handles the prior one.
            assert!(context.signal.signal(true));
        }
    }

    let context = Context {
        signal: MainSignal::default(),
        calls: AtomicUsize::new(0),
    };
    let mut source_context = CFRunLoopSourceContext {
        version: 0,
        info: ptr::from_ref(&context).cast_mut().cast(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(perform),
    };
    // SAFETY: the complete context points to `context`, retained for the test.
    let source = unsafe { CFRunLoopSource::new(None, 0, &raw mut source_context) }.unwrap();
    let run_loop = CFRunLoop::current().unwrap();
    // SAFETY: the mode is a process-wide constant.
    let mode = unsafe { kCFRunLoopDefaultMode };
    run_loop.add_source(Some(&source), mode);
    let _registration = Registration {
        source: &source,
        run_loop: &run_loop,
    };
    // SAFETY: neither the signal nor its callers outlive these retained objects.
    unsafe { context.signal.install(&source, &run_loop) };
    assert!(context.signal.signal(true));
    CFRunLoop::run_in_mode(mode, 0.0, true);
    assert_eq!(
        context.calls.load(Ordering::Acquire),
        0,
        "audio must not signal CoreFoundation"
    );

    for expected in 1..=2 {
        // Relay from another thread, just as the helper's control thread does.
        std::thread::scope(|scope| scope.spawn(|| context.signal.relay()).join().unwrap());
        CFRunLoop::run_in_mode(mode, 0.0, true);
        assert_eq!(context.calls.load(Ordering::Acquire), expected);
    }
    assert!(!context.signal.pending.load(Ordering::Acquire));
    assert!(!context.signal.signal(false));
    CFRunLoop::run_in_mode(mode, 0.0, true);
    assert_eq!(
        context.calls.load(Ordering::Acquire),
        3,
        "non-audio requests stay direct"
    );
}
