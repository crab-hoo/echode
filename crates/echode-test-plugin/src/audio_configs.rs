//! Port-configuration presets with non-index ids and lifecycle/rescan probes.
use crate::{TestGain, audio_ports, thread_check};
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, clap_audio_ports_config, clap_host_audio_ports_config,
    clap_plugin_audio_ports_config,
};
use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;

#[derive(Clone, Copy)]
struct HostAudioConfigs(RawExtension<HostExtensionSide, clap_host_audio_ports_config>);
// SAFETY: this identifier selects the official CLAP host configuration table.
unsafe impl Extension for HostAudioConfigs {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS_CONFIG];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: the identifier selects the clap_host_audio_ports_config layout.
        Self(unsafe { raw.cast() })
    }
}

pub(crate) fn probe(host: HostSharedHandle<'_>) -> bool {
    let Some(extension) = host.get_extension::<HostAudioConfigs>() else {
        return false;
    };
    // SAFETY: the host owns this table, and the lifecycle probes run on main.
    unsafe {
        let Some(rescan) = extension.0.as_ptr().as_ref().rescan else {
            return false;
        };
        rescan(extension.0.host_ptr().as_ptr());
    }
    true
}

#[derive(Clone, Copy)]
pub(crate) struct PluginAudioConfigs;
// SAFETY: this identifier selects the official plugin configuration table.
unsafe impl Extension for PluginAudioConfigs {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS_CONFIG];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: the implementation has the official C layout and callback signatures.
unsafe impl ExtensionImplementation<TestGain> for PluginAudioConfigs {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_audio_ports_config {
            count: Some(count),
            get: Some(get),
            select: Some(select),
        });
}

fn check(plugin: *const clap_plugin) -> bool {
    // SAFETY: each callback receives the live plugin on the host's main thread.
    unsafe {
        PluginWrapper::<TestGain>::handle(plugin, |wrapper| {
            Ok(
                thread_check::check(wrapper.main_thread().host.shared(), 1).is_ok()
                    && audio_ports::config_inactive(),
            )
        })
    }
    .unwrap_or(false)
}

unsafe extern "C" fn count(plugin: *const clap_plugin) -> u32 {
    if check(plugin) { 3 } else { 0 }
}

unsafe extern "C" fn get(
    plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_audio_ports_config,
) -> bool {
    if !check(plugin) || index >= 3 || info.is_null() {
        return false;
    }
    let (id, inputs, output_channels) = match index {
        0 => (100, 1, 2),
        1 => (206, 1, 1),
        _ => (303, 2, 2),
    };
    // SAFETY: the host provided writable storage for one descriptor.
    unsafe {
        *info = clap_audio_ports_config {
            id,
            name: audio_ports::name(),
            input_port_count: inputs,
            output_port_count: 1,
            has_main_input: true,
            main_input_channel_count: if index == 2 { 1 } else { 2 },
            main_input_port_type: std::ptr::null(),
            has_main_output: true,
            main_output_channel_count: output_channels,
            main_output_port_type: std::ptr::null(),
        };
    }
    true
}

unsafe extern "C" fn select(plugin: *const clap_plugin, id: u32) -> bool {
    if !check(plugin) {
        return false;
    }
    let layout = match id {
        100 => 0,
        206 => 6,
        303 => 3,
        _ => return false,
    };
    audio_ports::config_select(layout)
}
