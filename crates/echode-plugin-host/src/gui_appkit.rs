use super::{APPKIT, Surface};
use echode_sandbox::PluginFailure;
use objc2::rc::Retained;
use objc2::runtime::{AnyProtocol, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent, NSEventMask,
    NSEventModifierFlags, NSEventType, NSPanel, NSTextView, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSDate, NSDefaultRunLoopMode, NSNotFound, NSPoint, NSRange, NSRect, NSSize, NSString,
};
use std::cell::Cell;
use std::ffi::c_void;

#[derive(Default)]
struct PanelState {
    closed: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[ivars = PanelState]
    struct HostPanel;

    impl HostPanel {
        #[unsafe(method(close))]
        fn close_panel(&self) {
            self.ivars().closed.set(true);
            // SAFETY: invokes NSPanel's inherited close implementation.
            unsafe { msg_send![super(self), close] }
        }
    }
);

pub(super) struct Panel {
    mtm: MainThreadMarker,
    panel: Option<Retained<HostPanel>>,
    parent: libc::pid_t,
    active: Cell<bool>,
}

impl Panel {
    pub(super) fn new() -> Result<Self, PluginFailure> {
        let mtm = MainThreadMarker::new().ok_or(PluginFailure::PanelNeedsMainThread)?;
        if !APPKIT.with(Cell::get) {
            let app = NSApplication::sharedApplication(mtm);
            // No Dock icon and no menu bar: Echode stays the app the user works in.
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            app.finishLaunching();
            APPKIT.with(|started| started.set(true));
        }
        Ok(Self {
            mtm,
            panel: None,
            // SAFETY: `getppid` has no preconditions.
            parent: unsafe { libc::getppid() },
            active: Cell::new(false),
        })
    }
}

impl Surface for Panel {
    fn create(
        &mut self,
        title: &str,
        width: u32,
        height: u32,
        resizable: bool,
    ) -> Result<*mut c_void, PluginFailure> {
        let mut style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::NonactivatingPanel;
        if resizable {
            style |= NSWindowStyleMask::Resizable;
        }
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(f64::from(width), f64::from(height)),
        );
        let allocated = HostPanel::alloc(self.mtm).set_ivars(PanelState::default());
        // SAFETY: a standard initializer on a freshly allocated panel.
        let panel: Retained<HostPanel> = unsafe {
            msg_send![
                super(allocated),
                initWithContentRect: frame,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        // Above Echode's windows while Echode is active; never the app that is active.
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        // A click inside makes the panel key, so the plugin gets typing then.
        panel.setBecomesKeyOnlyIfNeeded(false);
        // SAFETY: the panel is kept in `self.panel` until `destroy`.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setTitle(&NSString::from_str(title));
        panel.center();
        let view = panel.contentView().ok_or(PluginFailure::PanelHasNoView)?;
        let pointer = Retained::as_ptr(&view).cast_mut().cast::<c_void>();
        self.panel = Some(panel);
        Ok(pointer)
    }

    fn order_front(&mut self, key: bool) {
        if let Some(panel) = &self.panel {
            if key {
                panel.makeKeyAndOrderFront(None);
            } else {
                // Without activating the helper or taking the keyboard.
                panel.orderFrontRegardless();
            }
        }
    }

    fn order_out(&mut self) {
        if let Some(panel) = &self.panel {
            panel.orderOut(None);
        }
    }

    fn destroy(&mut self) {
        if let Some(panel) = self.panel.take() {
            panel.close();
        }
    }

    fn is_key(&self) -> bool {
        self.panel.as_ref().is_some_and(|panel| panel.isKeyWindow())
    }

    fn user_closed(&self) -> bool {
        self.panel
            .as_ref()
            .is_some_and(|panel| panel.ivars().closed.get())
    }

    fn content_size(&self) -> Option<(u32, u32)> {
        let view = self.panel.as_ref()?.contentView()?;
        let size = view.frame().size;
        Some((size.width.round() as u32, size.height.round() as u32))
    }

    fn set_content_size(&mut self, width: u32, height: u32) {
        if let Some(panel) = &self.panel {
            panel.setContentSize(NSSize::new(f64::from(width), f64::from(height)));
        }
    }

    fn parent_active(&self) -> bool {
        if let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() {
            let foreground = app.processIdentifier();
            let active = workspace_active(self.parent, std::process::id(), foreground)
                || process_parent(foreground) == Some(self.parent);
            self.active.set(active);
        }
        self.active.get()
    }
}

/// A different plugin helper of the same Echode process is still part of
/// this workspace. This query runs only on the helper's main thread.
fn process_parent(pid: libc::pid_t) -> Option<libc::pid_t> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = size_of::<libc::proc_bsdinfo>();
    // SAFETY: a correctly sized writable proc_bsdinfo, using libc's public API.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if usize::try_from(read) != Ok(size) {
        return None;
    }
    // SAFETY: proc_pidinfo filled the entire structure on success.
    libc::pid_t::try_from(unsafe { info.assume_init() }.pbi_ppid).ok()
}

fn workspace_active(parent: libc::pid_t, helper: u32, foreground: libc::pid_t) -> bool {
    foreground == parent || u32::try_from(foreground) == Ok(helper)
}

/// Handles AppKit events for up to `seconds`, then returns to the caller's loop.
pub(super) fn run_once(seconds: f64, mut command: impl FnMut(echode_sandbox::PluginCommand)) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let until = NSDate::dateWithTimeIntervalSinceNow(seconds);
    // SAFETY: the default run-loop mode is a process-wide constant.
    let mode = unsafe { NSDefaultRunLoopMode };
    let mut wait = Some(until);
    while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
        NSEventMask::Any,
        wait.as_deref(),
        mode,
        true,
    ) {
        if let Some(shortcut) = shortcut(&event, mtm) {
            // Holding a transport key must not toggle on every repeat.
            if !event.isARepeat() {
                command(shortcut);
            }
        } else {
            app.sendEvent(&event);
        }
        // Drain what is already queued without waiting again.
        wait = Some(NSDate::distantPast());
    }
    app.updateWindows();
}

fn shortcut(event: &NSEvent, mtm: MainThreadMarker) -> Option<echode_sandbox::PluginCommand> {
    if event.r#type() != NSEventType::KeyDown {
        return None;
    }
    let window = event.window(mtm)?;
    if !window.isKeyWindow() {
        return None;
    }
    let editing = window.firstResponder().is_some_and(|responder| {
        if let Some(text) = responder.downcast_ref::<NSTextView>() {
            return text.isEditable();
        }
        // Custom Cocoa text clients (including plugin toolkit views) can
        // expose a caret without being an NSTextView. NSNotFound means
        // this client currently has no text selection or insertion point.
        AnyProtocol::get(c"NSTextInputClient").is_some_and(|protocol| {
            if !responder.conformsToProtocol(protocol)
                || !responder.respondsToSelector(sel!(selectedRange))
            {
                return false;
            }
            // SAFETY: selectedRange is the public NSTextInputClient method,
            // checked above; the responder lives for this main-thread call.
            let range: NSRange = unsafe { msg_send![&*responder, selectedRange] };
            range.location != NSNotFound as usize
        })
    });
    key_command(event, editing)
}

fn key_command(event: &NSEvent, editing: bool) -> Option<echode_sandbox::PluginCommand> {
    if event.r#type() != NSEventType::KeyDown {
        return None;
    }
    let flags = event.modifierFlags();
    let mut modifiers = 0;
    for (flag, bit) in [
        (NSEventModifierFlags::Command, 1),
        (NSEventModifierFlags::Shift, 2),
        (NSEventModifierFlags::Option, 4),
        (NSEventModifierFlags::Control, 8),
    ] {
        if flags.contains(flag) {
            modifiers |= bit;
        }
    }
    echode_sandbox::PluginCommand::from_key(event.keyCode(), modifiers, editing)
}

#[cfg(test)]
#[path = "gui_appkit_tests.rs"]
mod tests;
