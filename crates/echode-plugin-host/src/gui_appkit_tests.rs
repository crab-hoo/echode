use super::*;
use echode_sandbox::PluginCommand;

#[test]
fn focusing_the_helper_keeps_the_workspace_active() {
    assert!(workspace_active(10, 20, 10));
    assert!(workspace_active(10, 20, 20));
    assert!(!workspace_active(10, 20, 30));
    // SAFETY: getpid/getppid have no preconditions.
    assert_eq!(
        process_parent(unsafe { libc::getpid() }),
        Some(unsafe { libc::getppid() })
    );
}

/// Real AppKit key events, without a window or an audio device. The same
/// adapter is used before dispatching focused plugin input in run_once.
#[test]
fn contract_plugin_key_events_keep_typing_and_forward_workspace_shortcuts() {
    for (key, flags, expected) in [
        (
            49,
            NSEventModifierFlags::empty(),
            Some(PluginCommand::Playback),
        ),
        (6, NSEventModifierFlags::Command, Some(PluginCommand::Undo)),
        (
            6,
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            Some(PluginCommand::Redo),
        ),
        (47, NSEventModifierFlags::Command, Some(PluginCommand::Stop)),
        (49, NSEventModifierFlags::Shift, None),
        (0, NSEventModifierFlags::empty(), None),
    ] {
        for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                kind, NSPoint::new(0.0, 0.0), flags, 0.0, 0, None,
                &NSString::from_str(" "), &NSString::from_str(" "), false, key,
            ).expect("native key event");
            assert_eq!(
                key_command(&event, false),
                if kind == NSEventType::KeyDown {
                    expected
                } else {
                    None
                }
            );
            assert_eq!(
                key_command(&event, true),
                None,
                "text entry must keep its own input"
            );
        }
    }
}
