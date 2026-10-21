//! The DAW's existing global shortcuts while a plugin window has focus.
//! Modifiers use AppKit's device-independent bits, normalized by the helper.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PluginCommand {
    Playback,
    Stop,
    Undo,
    Redo,
    NewProject,
    OpenProject,
    AddTrack,
    Inspector,
    Browser,
    Editor,
    Palette,
}

impl PluginCommand {
    pub fn name(self) -> &'static str {
        match self {
            Self::Playback => "Play / Pause",
            Self::Stop => "Stop",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::NewProject => "New Project",
            Self::OpenProject => "Open Project",
            Self::AddTrack => "Add Track",
            Self::Inspector => "Toggle Inspector",
            Self::Browser => "Toggle Files",
            Self::Editor => "Toggle Note Editor",
            Self::Palette => "Command Palette",
        }
    }

    pub(crate) fn decode(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Playback,
            1 => Self::Stop,
            2 => Self::Undo,
            3 => Self::Redo,
            4 => Self::NewProject,
            5 => Self::OpenProject,
            6 => Self::AddTrack,
            7 => Self::Inspector,
            8 => Self::Browser,
            9 => Self::Editor,
            10 => Self::Palette,
            _ => return None,
        })
    }

    /// Physical macOS key codes, as in the DAW's current keyboard bindings.
    /// Text entry keeps Space, editing shortcuts and input-method commands.
    pub fn from_key(code: u16, modifiers: u8, editing: bool) -> Option<Self> {
        if editing {
            return None;
        }
        Some(match (code, modifiers) {
            (49, 0) => Self::Playback,
            (47, 1) => Self::Stop,
            (6, 1) => Self::Undo,
            (6, 3) => Self::Redo,
            (45, 1) => Self::NewProject,
            (31, 1) => Self::OpenProject,
            (17, 1) => Self::AddTrack,
            (18, 5) => Self::Inspector,
            (19, 5) => Self::Browser,
            (20, 5) => Self::Editor,
            (35, 3) => Self::Palette,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_workspace_bindings_leave_the_plugin() {
        assert_eq!(
            PluginCommand::from_key(49, 0, false),
            Some(PluginCommand::Playback)
        );
        assert_eq!(
            PluginCommand::from_key(6, 3, false),
            Some(PluginCommand::Redo)
        );
        for command in 0..=10 {
            assert_eq!(PluginCommand::decode(command).unwrap() as u8, command);
        }
        assert_eq!(PluginCommand::decode(11), None);
        for code in [49, 6, 45, 31, 17, 18, 19, 20, 35] {
            for modifiers in 0..16 {
                assert_eq!(PluginCommand::from_key(code, modifiers, true), None);
            }
        }
        assert_eq!(PluginCommand::from_key(49, 2, false), None);
        assert_eq!(PluginCommand::from_key(0, 0, false), None);
    }
}
