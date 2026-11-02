//! Device scalars shared by the model, package serde, and changesets.

// Declaration order is the package key order. Existing journals use separate
// creation and update orders; plugin identity is not included in the diff.
#[doc(hidden)]
#[macro_export]
macro_rules! device_fields {
    ($consumer:ident) => {
        $consumer! {
            fields {
                /// CLAP plugin id. The mixer leaves this empty.
                Plugin => plugin: Option<String>, text_value, field_optional_text, parse_plugin; text device_plugin;
                /// BLAKE3 of the plugin's saved bytes in `media/objects`. The mixer is `None`.
                PluginState => state: Option<String>, text_value, field_state, parse_state; skip none;
                /// Host bypass keeps the helper and its settings, while passing dry audio.
                Bypassed => bypassed: bool, flag_value, field_flag, parse_flag; copy device_bypassed;
                /// Offline devices keep their recorded settings without loading a helper.
                Offline => offline: bool, flag_value, field_flag, parse_flag; copy device_offline;
                /// CLAP parameter ids shown first, ascending and unique. The mixer has none.
                PinnedParameters => pinned: Vec<u32>, pins_value, field_pinned, parse_pins; slice device_pins;
            }
            create [Bypassed, Offline, PinnedParameters, Plugin, PluginState];
            diff [PinnedParameters, Offline, Bypassed, PluginState];
        }
    };
}

pub(crate) use device_fields;
