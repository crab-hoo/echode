//! Explicit, control-only audio-port preset and configurable-port queries.
use crate::extension::{table, table_with_compat};
use crate::threads::EchodeHost;
use clack_host::extensions::prelude::*;
use clap_sys::ext::ambisonic::{CLAP_PORT_AMBISONIC, clap_ambisonic_config};
use clap_sys::ext::audio_ports::{CLAP_PORT_MONO, CLAP_PORT_STEREO};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, CLAP_EXT_AUDIO_PORTS_CONFIG_INFO,
    CLAP_EXT_AUDIO_PORTS_CONFIG_INFO_COMPAT, clap_audio_ports_config, clap_host_audio_ports_config,
    clap_plugin_audio_ports_config, clap_plugin_audio_ports_config_info,
};
use clap_sys::ext::configurable_audio_ports::{
    CLAP_EXT_CONFIGURABLE_AUDIO_PORTS, CLAP_EXT_CONFIGURABLE_AUDIO_PORTS_COMPAT,
    clap_audio_port_configuration_request, clap_plugin_configurable_audio_ports,
};
use clap_sys::ext::surround::CLAP_PORT_SURROUND;
use clap_sys::host::clap_host;
use clap_sys::id::CLAP_INVALID_ID;
use clap_sys::plugin::clap_plugin;
use echode_sandbox::{
    AudioPortConfig, AudioPortConfigs, AudioPortDetails, AudioPortRequest, PluginFailure,
    PortLayout,
};
use std::collections::HashSet;
use std::ffi::{CStr, c_char};
use std::mem::MaybeUninit;
use std::ptr;

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
pub(crate) fn read(plugin: &clap_plugin) -> Result<AudioPortConfigs, PluginFailure> {
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
            ports: None,
        });
    }
    let current_id = read_config_info(plugin, &ids, &mut configs)?;
    Ok(AudioPortConfigs {
        current_id,
        configs,
    })
}

fn read_config_info(
    plugin: &clap_plugin,
    ids: &HashSet<u32>,
    configs: &mut [AudioPortConfig],
) -> Result<Option<u32>, PluginFailure> {
    // SAFETY: `clap_plugin_audio_ports_config_info` is the table for both identifiers.
    let Some(info_ext) = (unsafe {
        table_with_compat::<clap_plugin_audio_ports_config_info>(
            plugin,
            CLAP_EXT_AUDIO_PORTS_CONFIG_INFO,
            CLAP_EXT_AUDIO_PORTS_CONFIG_INFO_COMPAT,
        )
    }) else {
        return Ok(None);
    };
    let current_config = info_ext
        .current_config
        .ok_or(PluginFailure::NoAudioConfigCurrent)?;
    let get = info_ext.get.ok_or(PluginFailure::NoAudioConfigPortInfo)?;
    // SAFETY: this callback runs on main while the plugin is deactivated.
    let current = unsafe { current_config(plugin) };
    let current_id = if current == CLAP_INVALID_ID {
        None
    } else if ids.contains(&current) {
        Some(current)
    } else {
        return Err(PluginFailure::AudioConfigUndescribed);
    };
    for config in configs {
        let read_side = |count, is_input| {
            crate::audio_ports::read_port_channels(
                count,
                |index, port_info| {
                    // SAFETY: main-thread callback while inactive, with descriptor storage.
                    unsafe { get(plugin, config.id, index, is_input, port_info) }
                },
                |_| PluginFailure::AudioConfigUndescribed,
                |_| PluginFailure::AudioConfigUndescribed,
            )
        };
        let inputs = read_side(config.input_ports, true)?;
        let outputs = read_side(config.output_ports, false)?;
        if (config.main_input_channels > 0
            && inputs.first().copied() != Some(config.main_input_channels))
            || (config.main_output_channels > 0
                && outputs.first().copied() != Some(config.main_output_channels))
        {
            return Err(PluginFailure::AudioConfigUndescribed);
        }
        config.ports = Some(PortLayout::new(inputs, outputs));
    }
    Ok(current_id)
}

/// Selection is by id, not index. Always reread audio-ports before reactivation,
/// including after a refusal: never assume a plugin preserved its old shape.
pub(crate) fn select(plugin: &clap_plugin, id: u32) -> Result<(), PluginFailure> {
    if id == CLAP_INVALID_ID {
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

fn configurable_extension(
    plugin: &clap_plugin,
) -> Result<&clap_plugin_configurable_audio_ports, PluginFailure> {
    // SAFETY: `clap_plugin_configurable_audio_ports` is the table for both identifiers.
    unsafe {
        table_with_compat(
            plugin,
            CLAP_EXT_CONFIGURABLE_AUDIO_PORTS,
            CLAP_EXT_CONFIGURABLE_AUDIO_PORTS_COMPAT,
        )
    }
    .ok_or(PluginFailure::NoConfigurableAudioPorts)
}

fn port_type_ptr(channels: u32) -> *const c_char {
    match channels {
        1 => CLAP_PORT_MONO.as_ptr(),
        2 => CLAP_PORT_STEREO.as_ptr(),
        _ => ptr::null(),
    }
}

fn invoke_requests(
    plugin: &clap_plugin,
    requests: &[AudioPortRequest],
    callback: unsafe extern "C" fn(
        *const clap_plugin,
        *const clap_audio_port_configuration_request,
        u32,
    ) -> bool,
) -> Result<bool, PluginFailure> {
    let count =
        u32::try_from(requests.len()).map_err(|_| PluginFailure::ConfigurableAudioPortsRefused)?;
    // Own every ABI-specific value before building pointers; neither vector moves
    // or grows during the callback. Surround maps remain borrowed from `requests`.
    let ambisonic: Vec<_> = requests
        .iter()
        .map(|request| match request.details {
            Some(AudioPortDetails::Ambisonic {
                ordering,
                normalization,
            }) => clap_ambisonic_config {
                ordering,
                normalization,
            },
            _ => clap_ambisonic_config {
                ordering: 0,
                normalization: 0,
            },
        })
        .collect();
    let c_requests: Vec<_> = requests
        .iter()
        .zip(&ambisonic)
        .map(|(request, ambisonic)| {
            let (port_type, port_details) = match &request.details {
                None => (port_type_ptr(request.channels), ptr::null()),
                Some(AudioPortDetails::Surround(map)) => {
                    // CLAP reads channel_count bytes. Reject a short OR long map
                    // before lending any pointer, including the empty map case.
                    if map.len() != request.channels as usize || map.is_empty() {
                        return Err(PluginFailure::ConfigurableAudioPortsRefused);
                    }
                    (CLAP_PORT_SURROUND.as_ptr(), map.as_ptr().cast())
                }
                Some(AudioPortDetails::Ambisonic { .. }) => (
                    CLAP_PORT_AMBISONIC.as_ptr(),
                    ptr::from_ref(ambisonic).cast(),
                ),
            };
            Ok(clap_audio_port_configuration_request {
                is_input: request.input,
                port_index: request.index,
                channel_count: request.channels,
                port_type,
                port_details,
            })
        })
        .collect::<Result<_, _>>()?;
    let ptr = if c_requests.is_empty() {
        ptr::null()
    } else {
        c_requests.as_ptr()
    };
    // SAFETY: `c_requests` outlives the call on main while the plugin is deactivated.
    Ok(unsafe { callback(plugin, ptr, count) })
}

/// Ask whether the plugin can apply `requests` while deactivated.
pub(crate) fn can_apply(
    plugin: &clap_plugin,
    requests: &[AudioPortRequest],
) -> Result<bool, PluginFailure> {
    let callback = configurable_extension(plugin)?
        .can_apply_configuration
        .ok_or(PluginFailure::NoConfigurableAudioPortsCanApply)?;
    invoke_requests(plugin, requests, callback)
}

/// Apply `requests` while deactivated. Always reread audio-ports before reactivation.
pub(crate) fn apply(
    plugin: &clap_plugin,
    requests: &[AudioPortRequest],
) -> Result<(), PluginFailure> {
    let callback = configurable_extension(plugin)?
        .apply_configuration
        .ok_or(PluginFailure::NoConfigurableAudioPortsApply)?;
    if invoke_requests(plugin, requests, callback)? {
        Ok(())
    } else {
        Err(PluginFailure::ConfigurableAudioPortsRefused)
    }
}

#[cfg(test)]
#[path = "audio_configs_tests.rs"]
mod tests;
