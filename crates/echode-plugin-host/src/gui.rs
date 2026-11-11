//! The plugin's own window, owned by the helper on its main thread
//! (ADR 0041, ADR 0132).
//!
//! Showing or raising the window never makes it the key window: Echode keeps
//! the keyboard until the user clicks inside the plugin, and the panel never
//! activates the helper, so Echode's menu bar stays. The window floats above
//! Echode while Echode is the active app, hides while another app is active,
//! and comes back when Echode is active again.
//!
//! `ECHODE_TEST_FAULT=gui:<socket>` replaces AppKit with a recorded window, so
//! tests check these rules without opening a window on screen.

use clap_sys::ext::gui::{
    CLAP_EXT_GUI, CLAP_WINDOW_API_COCOA, clap_plugin_gui, clap_window, clap_window_handle,
};
use clap_sys::plugin::clap_plugin;
use echode_sandbox::{GuiReport, PluginFailure};
use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What the window must do after a request or a change of the active app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowAction {
    Create,
    /// Brings the window to the front. `key` would also give it the keyboard.
    OrderFront {
        key: bool,
    },
    OrderOut,
    Destroy,
}

/// Open, raise and hide rules, apart from AppKit so they are tested directly.
#[derive(Default)]
pub(crate) struct WindowPolicy {
    open: bool,
    /// On screen. An open window is off screen while Echode is not active.
    visible: bool,
}

impl WindowPolicy {
    /// Opens the window, or raises it when it is open. Neither takes focus.
    /// While another app is active the window waits off screen.
    pub(crate) fn show(&mut self, parent_active: bool) -> Vec<WindowAction> {
        let mut actions = Vec::new();
        if !self.open {
            self.open = true;
            actions.push(WindowAction::Create);
        }
        if parent_active {
            self.visible = true;
            actions.push(WindowAction::OrderFront { key: false });
        }
        actions
    }

    /// Follows Echode becoming active or inactive.
    pub(crate) fn parent(&mut self, active: bool) -> Vec<WindowAction> {
        if !self.open || active == self.visible {
            return Vec::new();
        }
        self.visible = active;
        vec![if active {
            WindowAction::OrderFront { key: false }
        } else {
            WindowAction::OrderOut
        }]
    }

    /// Closes on request, or after the user or the plugin closed the window.
    pub(crate) fn close(&mut self) -> Vec<WindowAction> {
        if !self.open {
            return Vec::new();
        }
        self.open = false;
        self.visible = false;
        vec![WindowAction::Destroy]
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }
}

/// Requests the plugin makes from any thread. The main thread applies them.
#[derive(Default)]
pub(crate) struct GuiRequests {
    show: AtomicBool,
    hide: AtomicBool,
    closed: AtomicBool,
    resize: AtomicU64,
    resize_pending: AtomicBool,
}

impl GuiRequests {
    pub(crate) fn request_show(&self) {
        self.show.store(true, Ordering::Release);
    }

    pub(crate) fn request_hide(&self) {
        self.hide.store(true, Ordering::Release);
    }

    pub(crate) fn closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(crate) fn request_resize(&self, width: u32, height: u32) {
        self.resize.store(
            u64::from(width) << 32 | u64::from(height),
            Ordering::Release,
        );
        self.resize_pending.store(true, Ordering::Release);
    }
}

/// The window the plugin draws into.
trait Surface {
    /// Creates the window off screen and returns its content view.
    fn create(
        &mut self,
        title: &str,
        width: u32,
        height: u32,
        resizable: bool,
    ) -> Result<*mut c_void, PluginFailure>;
    fn order_front(&mut self, key: bool);
    fn order_out(&mut self);
    fn destroy(&mut self);
    fn is_key(&self) -> bool;
    /// An explicit close, independent of temporary OS visibility changes.
    fn user_closed(&self) -> bool;
    fn content_size(&self) -> Option<(u32, u32)>;
    fn set_content_size(&mut self, width: u32, height: u32);
    /// Echode or a helper from the same parent is the active workspace.
    fn parent_active(&self) -> bool;
}

struct GuiHost {
    policy: WindowPolicy,
    surface: Box<dyn Surface>,
    /// The plugin made its own floating window; the helper has no panel.
    floating: bool,
    resizable: bool,
    size: (u32, u32),
}

thread_local! {
    static GUI: RefCell<Option<GuiHost>> = const { RefCell::new(None) };
    /// AppKit was started for a window, so the run loop also handles events.
    static APPKIT: Cell<bool> = const { Cell::new(false) };
}

fn extension(plugin: &clap_plugin) -> Option<&clap_plugin_gui> {
    // SAFETY: `clap_plugin_gui` is the table for `CLAP_EXT_GUI`.
    unsafe { crate::extension::table::<clap_plugin_gui>(plugin, CLAP_EXT_GUI) }
}

fn recorded() -> bool {
    std::env::var("ECHODE_TEST_FAULT").is_ok_and(|fault| fault.starts_with("gui:"))
}

/// Opens or raises the plugin's window. Main thread only.
pub(crate) fn show(plugin: &clap_plugin, title: &str) -> Result<GuiReport, PluginFailure> {
    let gui = extension(plugin).ok_or(PluginFailure::NoWindow)?;
    GUI.with(|cell| {
        let mut slot = cell.borrow_mut();
        let created = slot.as_ref().is_none_or(|host| !host.policy.is_open());
        let host = match slot.as_mut() {
            Some(host) if !created => host,
            _ => slot.insert(open(plugin, gui, title)?),
        };
        let actions = host.policy.show(host.surface.parent_active());
        apply(plugin, gui, host, &actions);
        Ok(GuiReport {
            created,
            visible: host.policy.is_visible(),
            key: host.surface.is_key(),
        })
    })
}

/// Closes the plugin's window. Closing when nothing is open succeeds.
pub(crate) fn close(plugin: &clap_plugin) {
    let Some(gui) = extension(plugin) else {
        return;
    };
    GUI.with(|cell| {
        if let Some(host) = cell.borrow_mut().as_mut() {
            let actions = host.policy.close();
            apply(plugin, gui, host, &actions);
        }
    });
}

/// Applies plugin requests and follows the active app. Returns true when the
/// user or the plugin closed the window, which the parent is told about.
pub(crate) fn tick(plugin: &clap_plugin, requests: &GuiRequests) -> bool {
    let Some(gui) = extension(plugin) else {
        return false;
    };
    GUI.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(host) = slot.as_mut().filter(|host| host.policy.is_open()) else {
            requests.show.store(false, Ordering::Release);
            requests.hide.store(false, Ordering::Release);
            requests.closed.store(false, Ordering::Release);
            return false;
        };
        let closed_by_user = !host.floating && host.surface.user_closed();
        if closed_by_user
            || requests.closed.swap(false, Ordering::AcqRel)
            || requests.hide.swap(false, Ordering::AcqRel)
        {
            let actions = host.policy.close();
            apply(plugin, gui, host, &actions);
            return true;
        }
        if requests.show.swap(false, Ordering::AcqRel) {
            let actions = host.policy.show(host.surface.parent_active());
            apply(plugin, gui, host, &actions);
        }
        let actions = host.policy.parent(host.surface.parent_active());
        apply(plugin, gui, host, &actions);
        if requests.resize_pending.swap(false, Ordering::AcqRel) && !host.floating {
            let packed = requests.resize.load(Ordering::Acquire);
            resize(plugin, gui, host, (packed >> 32) as u32, packed as u32);
        } else if host.resizable
            && let Some((width, height)) = host.surface.content_size()
            && (width, height) != host.size
        {
            resize(plugin, gui, host, width, height);
        }
        false
    })
}

/// Destroys the plugin's view before the plugin is deactivated.
pub(crate) fn shut_down(plugin: &clap_plugin) {
    close(plugin);
    GUI.with(|cell| cell.borrow_mut().take());
}

/// Runs the main run loop once. After a window was created, AppKit events are
/// handled in the same loop (ADR 0041).
pub(crate) fn run_once(seconds: f64, command: impl FnMut(echode_sandbox::PluginCommand)) {
    if APPKIT.with(Cell::get) {
        appkit::run_once(seconds, command);
    } else {
        // SAFETY: the default-mode symbol is a process-wide constant.
        let mode = unsafe { objc2_core_foundation::kCFRunLoopDefaultMode };
        let _ = objc2_core_foundation::CFRunLoop::run_in_mode(mode, seconds, true);
    }
}

fn open(
    plugin: &clap_plugin,
    gui: &clap_plugin_gui,
    title: &str,
) -> Result<GuiHost, PluginFailure> {
    let supported = |floating: bool| {
        gui.is_api_supported.is_some_and(|supported| {
            // SAFETY: main thread; the API name is a static C string.
            unsafe { supported(plugin, CLAP_WINDOW_API_COCOA.as_ptr(), floating) }
        })
    };
    let floating = if supported(false) {
        false
    } else if supported(true) {
        true
    } else {
        return Err(PluginFailure::NoCocoaWindow);
    };
    let create = gui.create.ok_or(PluginFailure::NoWindowCreate)?;
    // SAFETY: main thread, after the plugin was created.
    if !unsafe { create(plugin, CLAP_WINDOW_API_COCOA.as_ptr(), floating) } {
        return Err(PluginFailure::WindowRefused);
    }
    let mut surface: Box<dyn Surface> = if recorded() {
        Box::new(Recorded::default())
    } else {
        Box::new(appkit::Panel::new()?)
    };
    let mut host_size = (0, 0);
    let mut resizable = false;
    if floating {
        if let (Some(suggest), Ok(title)) = (gui.suggest_title, CString::new(title)) {
            // SAFETY: main thread; the title lives for the call.
            unsafe { suggest(plugin, title.as_ptr()) };
        }
    } else {
        host_size = size_of(plugin, gui).unwrap_or((400, 300));
        // SAFETY: main thread, after `create`.
        resizable = gui.can_resize.is_some_and(|can| unsafe { can(plugin) });
        let attached = surface
            .create(title, host_size.0, host_size.1, resizable)
            .and_then(|view| {
                let window = clap_window {
                    api: CLAP_WINDOW_API_COCOA.as_ptr(),
                    specific: clap_window_handle { cocoa: view },
                };
                let set_parent = gui.set_parent.ok_or(PluginFailure::NoEmbedding)?;
                // SAFETY: main thread; the view outlives the plugin's window.
                if unsafe { set_parent(plugin, &window) } {
                    Ok(())
                } else {
                    Err(PluginFailure::ParentRefused)
                }
            });
        if let Err(error) = attached {
            if let Some(destroy) = gui.destroy {
                // SAFETY: main thread, after a successful `create`.
                unsafe { destroy(plugin) };
            }
            surface.destroy();
            return Err(error);
        }
    }
    Ok(GuiHost {
        policy: WindowPolicy::default(),
        surface,
        floating,
        resizable,
        size: host_size,
    })
}

fn apply(
    plugin: &clap_plugin,
    gui: &clap_plugin_gui,
    host: &mut GuiHost,
    actions: &[WindowAction],
) {
    for action in actions {
        match *action {
            // `open` already created the plugin's view and the window.
            WindowAction::Create => {}
            WindowAction::OrderFront { key } => {
                if !host.floating {
                    host.surface.order_front(key);
                }
                if let Some(show) = gui.show {
                    // SAFETY: main thread, after `set_parent` or `suggest_title`.
                    unsafe { show(plugin) };
                }
            }
            WindowAction::OrderOut => {
                if let Some(hide) = gui.hide {
                    // SAFETY: main thread, while the view exists.
                    unsafe { hide(plugin) };
                }
                if !host.floating {
                    host.surface.order_out();
                }
            }
            WindowAction::Destroy => {
                if let Some(hide) = gui.hide {
                    // SAFETY: main thread, while the view exists.
                    unsafe { hide(plugin) };
                }
                if let Some(destroy) = gui.destroy {
                    // SAFETY: main thread; the plugin removes its view first.
                    unsafe { destroy(plugin) };
                }
                host.surface.destroy();
            }
        }
    }
}

fn size_of(plugin: &clap_plugin, gui: &clap_plugin_gui) -> Option<(u32, u32)> {
    let get = gui.get_size?;
    let (mut width, mut height) = (0, 0);
    // SAFETY: main thread, after `create`; both outputs are writable.
    (unsafe { get(plugin, &mut width, &mut height) } && width > 0 && height > 0)
        .then_some((width, height))
}

/// Points, not pixels: CLAP's Cocoa windows use the screen's own scale.
fn resize(
    plugin: &clap_plugin,
    gui: &clap_plugin_gui,
    host: &mut GuiHost,
    width: u32,
    height: u32,
) {
    let (mut width, mut height) = (width.max(1), height.max(1));
    if let Some(adjust) = gui.adjust_size {
        // SAFETY: main thread; both values are writable.
        unsafe { adjust(plugin, &mut width, &mut height) };
    }
    if let Some(set) = gui.set_size {
        // SAFETY: main thread, while the view exists.
        unsafe { set(plugin, width, height) };
    }
    host.size = (width, height);
    if host.surface.content_size() != Some(host.size) {
        host.surface.set_content_size(width, height);
    }
}

/// A window that is never drawn. It records what the host did with it.
#[derive(Default)]
struct Recorded {
    open: bool,
    visible: bool,
    key: bool,
    size: (u32, u32),
    /// A stable address for the content view. Nothing dereferences it.
    view: Box<u8>,
}

impl Surface for Recorded {
    fn create(
        &mut self,
        _: &str,
        width: u32,
        height: u32,
        _: bool,
    ) -> Result<*mut c_void, PluginFailure> {
        self.open = true;
        self.size = (width, height);
        Ok(std::ptr::from_mut(&mut *self.view).cast())
    }

    fn order_front(&mut self, key: bool) {
        self.visible = self.open;
        self.key = self.open && key;
    }

    fn order_out(&mut self) {
        self.visible = false;
        self.key = false;
    }

    fn destroy(&mut self) {
        self.open = false;
        self.order_out();
    }

    fn is_key(&self) -> bool {
        self.key
    }

    fn user_closed(&self) -> bool {
        false
    }

    fn content_size(&self) -> Option<(u32, u32)> {
        self.open.then_some(self.size)
    }

    fn set_content_size(&mut self, width: u32, height: u32) {
        self.size = (width, height);
    }

    fn parent_active(&self) -> bool {
        true
    }
}

#[path = "gui_appkit.rs"]
mod appkit;

#[cfg(test)]
#[path = "gui_tests.rs"]
mod tests;
