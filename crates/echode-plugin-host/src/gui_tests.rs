use super::*;

/// AppKit can order a panel out without the user closing it. Visibility
/// must never stand in for the close notification and destroy the view.
#[test]
fn contract_plugin_window_survives_temporary_invisibility() {
    // SAFETY: every field of `clap_plugin_gui` is a nullable function pointer, for which all zero
    // bytes are `None`.
    static TABLE: clap_plugin_gui = unsafe { std::mem::zeroed() };
    unsafe extern "C" fn get(_: *const clap_plugin, _: *const std::ffi::c_char) -> *const c_void {
        std::ptr::addr_of!(TABLE).cast()
    }
    // All fields are nullable function/data pointers in the raw CLAP ABI.
    // SAFETY: every field of `clap_plugin` is a nullable pointer or a plain value, for which all
    // zero bytes are valid.
    let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
    plugin.get_extension = Some(get);
    let mut surface = Recorded::default();
    surface.create("test", 320, 200, false).unwrap();
    surface.order_front(false);
    let mut policy = WindowPolicy::default();
    policy.show(true);
    // Simulate an OS visibility change, rather than a user close.
    surface.order_out();
    GUI.with(|cell| {
        *cell.borrow_mut() = Some(GuiHost {
            policy,
            surface: Box::new(surface),
            floating: false,
            resizable: false,
            size: (320, 200),
        })
    });
    let closed = tick(&plugin, &GuiRequests::default());
    let open = GUI.with(|cell| cell.borrow_mut().take().unwrap().policy.is_open());
    assert!(!closed, "temporary invisibility must not report a close");
    assert!(open, "the plugin view must remain alive");
}

#[test]
fn showing_and_raising_never_take_focus() {
    let mut policy = WindowPolicy::default();
    assert_eq!(
        policy.show(true),
        [
            WindowAction::Create,
            WindowAction::OrderFront { key: false }
        ]
    );
    assert_eq!(policy.show(true), [WindowAction::OrderFront { key: false }]);
    assert!(policy.is_open() && policy.is_visible());
}

#[test]
fn the_window_follows_echode_being_active() {
    let mut policy = WindowPolicy::default();
    // Opened while another app is active: created, but kept off screen.
    assert_eq!(policy.show(false), [WindowAction::Create]);
    assert!(!policy.is_visible());
    assert_eq!(
        policy.parent(true),
        [WindowAction::OrderFront { key: false }]
    );
    assert!(policy.parent(true).is_empty());
    assert_eq!(policy.parent(false), [WindowAction::OrderOut]);
    assert_eq!(policy.close(), [WindowAction::Destroy]);
    assert!(policy.close().is_empty());
    assert!(
        policy.parent(true).is_empty(),
        "a closed window stays closed"
    );
}
