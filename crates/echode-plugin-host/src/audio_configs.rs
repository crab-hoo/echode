//! Explicit, control-only audio-port preset queries and selections.
use crate::extension::table;
use crate::threads::EchodeHost;
use clack_host::extensions::prelude::*;
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, clap_audio_ports_config, clap_host_audio_ports_config,
    clap_plugin_audio_ports_config,
};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use echode_sandbox::{AudioPortConfig, PluginFailure};
use std::collections::HashSet;
use std::ffi::CStr;
use std::mem::MaybeUninit;

#[derive(Clone, Copy)]
pub(crate) struct HostAudioConfigs;
// SAFETY: this identifier selects the official host configuration table.
unsafe impl Extension for HostAudioConfigs {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS_CONFIG];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: the implementation has the official C layout and callback signature.
unsafe impl ExtensionImplementation<EchodeHost> for HostAudioConfigs {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_host_audio_ports_config {
            rescan: Some(rescan),
        });
}
unsafe extern "C" fn rescan(_host: *const clap_host) {
    // No list is cached. The next explicit query scans while inactive; do not
    // reenter the plugin here or restart audio for a menu that is not open.
}

fn extension(plugin: &clap_plugin) -> Result<&clap_plugin_audio_ports_config, PluginFailure> {
    // SAFETY: this identifier selects the official plugin configuration table.
    unsafe { table(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG) }.ok_or(PluginFailure::NoAudioConfigs)
}

/// Main thread only, after audio hands back its processor and deactivate returns.
pub(crate) fn read(plugin: &clap_plugin) -> Result<Vec<AudioPortConfig>, PluginFailure> {
    let ports = extension(plugin)?;
    let count = ports.count.ok_or(PluginFailure::NoAudioConfigCount)?;
    let get = ports.get.ok_or(PluginFailure::NoAudioConfigInfo)?;
    let mut configs = Vec::new();
    let mut ids = HashSet::new();
    // SAFETY: these are the plugin's callbacks, invoked on main while inactive.
    for index in 0..unsafe { count(plugin) } {
        let mut info = MaybeUninit::<clap_audio_ports_config>::zeroed();
        // SAFETY: the plugin receives writable storage for one descriptor.
        if !unsafe { get(plugin, index, info.as_mut_ptr()) } {
            return Err(PluginFailure::AudioConfigUndescribed);
        }
        // SAFETY: zero is valid for every field and the callback filled the record.
        let info = unsafe { info.assume_init() };
        if info.id == u32::MAX
            || !ids.insert(info.id)
            || (info.has_main_input
                && (info.input_port_count == 0 || info.main_input_channel_count == 0))
            || (info.has_main_output
                && (info.output_port_count == 0 || info.main_output_channel_count == 0))
        {
            return Err(PluginFailure::AudioConfigUndescribed);
        }
        // Only bounded inline text is copied. Optional port-type pointers are
        // hints, not the exact layout that audio-ports must describe after select.
        let name: Vec<u8> = info
            .name
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        configs.push(AudioPortConfig {
            id: info.id,
            name: String::from_utf8_lossy(&name).into_owned(),
            input_ports: info.input_port_count,
            output_ports: info.output_port_count,
            main_input_channels: if info.has_main_input {
                info.main_input_channel_count
            } else {
                0
            },
            main_output_channels: if info.has_main_output {
                info.main_output_channel_count
            } else {
                0
            },
        });
    }
    Ok(configs)
}

/// Selection is by id, not index. Always reread audio-ports before reactivation,
/// including after a refusal: never assume a plugin preserved its old shape.
pub(crate) fn select(plugin: &clap_plugin, id: u32) -> Result<(), PluginFailure> {
    if id == u32::MAX {
        return Err(PluginFailure::AudioConfigRefused);
    }
    let select = extension(plugin)?
        .select
        .ok_or(PluginFailure::NoAudioConfigSelect)?;
    // SAFETY: this callback runs on main while the plugin is deactivated.
    if unsafe { select(plugin, id) } {
        Ok(())
    } else {
        Err(PluginFailure::AudioConfigRefused)
    }
}

#[cfg(test)]
#[path = "audio_configs_tests.rs"]
mod tests;
