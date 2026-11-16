//! Port-configuration presets, config info, and configurable-audio-ports probes.
use crate::{TestGain, audio_ports, thread_check};
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::audio_ports::{CLAP_PORT_MONO, CLAP_PORT_STEREO, clap_audio_port_info};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, CLAP_EXT_AUDIO_PORTS_CONFIG_INFO, clap_audio_ports_config,
    clap_host_audio_ports_config, clap_plugin_audio_ports_config,
    clap_plugin_audio_ports_config_info,
};
use clap_sys::ext::configurable_audio_ports::{
    CLAP_EXT_CONFIGURABLE_AUDIO_PORTS, clap_audio_port_configuration_request,
    clap_plugin_configurable_audio_ports,
};
use clap_sys::id::CLAP_INVALID_ID;
use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;

const PRESETS: [(u32, u8); 3] = [(100, 0), (206, 6), (303, 3)];

fn preset_layout(id: u32) -> Option<u8> {
    PRESETS
        .iter()
        .find_map(|&(preset_id, layout)| (preset_id == id).then_some(layout))
}

pub(crate) fn layout_preset(layout: u8) -> u32 {
    PRESETS
        .iter()
        .find_map(|&(preset_id, preset_layout)| (preset_layout == layout).then_some(preset_id))
        .unwrap_or(CLAP_INVALID_ID)
}

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

#[derive(Clone, Copy)]
pub(crate) struct PluginAudioConfigsInfo;
// SAFETY: this identifier selects the official plugin configuration-info table.
unsafe impl Extension for PluginAudioConfigsInfo {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS_CONFIG_INFO];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: the implementation has the official C layout and callback signatures.
unsafe impl ExtensionImplementation<TestGain> for PluginAudioConfigsInfo {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_audio_ports_config_info {
            current_config: Some(current_config),
            get: Some(info_get),
        });
}

#[derive(Clone, Copy)]
pub(crate) struct PluginConfigurableAudioPorts;
// SAFETY: this identifier selects the official plugin configurable-ports table.
unsafe impl Extension for PluginConfigurableAudioPorts {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_CONFIGURABLE_AUDIO_PORTS];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: the implementation has the official C layout and callback signatures.
unsafe impl ExtensionImplementation<TestGain> for PluginConfigurableAudioPorts {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_configurable_audio_ports {
            can_apply_configuration: Some(can_apply),
            apply_configuration: Some(apply),
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
    if check(plugin) {
        u32::try_from(PRESETS.len()).unwrap_or(0)
    } else {
        0
    }
}

unsafe extern "C" fn get(
    plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_audio_ports_config,
) -> bool {
    if !check(plugin) || info.is_null() {
        return false;
    }
    let Some(&(id, layout)) = PRESETS.get(index as usize) else {
        return false;
    };
    let (inputs, outputs) = audio_ports::layout_ports(layout);
    // SAFETY: the host provided writable storage for one descriptor.
    unsafe {
        *info = clap_audio_ports_config {
            id,
            name: audio_ports::name(),
            input_port_count: u32::try_from(inputs.len()).unwrap_or(0),
            output_port_count: u32::try_from(outputs.len()).unwrap_or(0),
            has_main_input: !inputs.is_empty(),
            main_input_channel_count: inputs.first().copied().unwrap_or(0),
            main_input_port_type: std::ptr::null(),
            has_main_output: !outputs.is_empty(),
            main_output_channel_count: outputs.first().copied().unwrap_or(0),
            main_output_port_type: std::ptr::null(),
        };
    }
    true
}

unsafe extern "C" fn select(plugin: *const clap_plugin, id: u32) -> bool {
    if !check(plugin) {
        return false;
    }
    let Some(layout) = preset_layout(id) else {
        return false;
    };
    audio_ports::config_select(layout, id)
}

unsafe extern "C" fn current_config(plugin: *const clap_plugin) -> u32 {
    if !check(plugin) {
        return CLAP_INVALID_ID;
    }
    audio_ports::config_current()
}

unsafe extern "C" fn info_get(
    plugin: *const clap_plugin,
    config_id: u32,
    port_index: u32,
    is_input: bool,
    config: *mut clap_audio_port_info,
) -> bool {
    if !check(plugin) || config.is_null() {
        return false;
    }
    let Some(layout) = preset_layout(config_id) else {
        return false;
    };
    let (inputs, outputs) = audio_ports::layout_ports(layout);
    let ports = if is_input { inputs } else { outputs };
    let Some(&channels) = ports.get(port_index as usize) else {
        return false;
    };
    // SAFETY: the host provided writable storage for one descriptor.
    audio_ports::fill_port_info(port_index, channels, unsafe { &mut *config });
    true
}

unsafe fn resolve_requests(
    plugin: *const clap_plugin,
    requests: *const clap_audio_port_configuration_request,
    request_count: u32,
) -> Option<(u8, u8)> {
    if !check(plugin) || request_count == 0 || requests.is_null() {
        return None;
    }
    // SAFETY: `requests` points to `request_count` readable descriptors from the host.
    let requests = unsafe { std::slice::from_raw_parts(requests, request_count as usize) };
    let (cur_in, cur_out) = audio_ports::layout_ports(audio_ports::current_layout());
    let mut next_in = cur_in.to_vec();
    let mut next_out = cur_out.to_vec();
    let mut details = 0;
    for req in requests {
        if !req.port_details.is_null() {
            // This fixture deliberately accepts just two known detail values.
            // SAFETY: the host lends the type-specific data for this callback.
            let kind = unsafe { CStr::from_ptr(req.port_type) };
            let valid = if kind == clap_sys::ext::surround::CLAP_PORT_SURROUND {
                // SAFETY: surround details contain channel_count readable bytes.
                let map = unsafe {
                    std::slice::from_raw_parts(
                        req.port_details.cast::<u8>(),
                        req.channel_count as usize,
                    )
                };
                details = 1;
                req.channel_count == 1 && map == [2]
            } else if kind == clap_sys::ext::ambisonic::CLAP_PORT_AMBISONIC {
                // SAFETY: ambisonic details contain one aligned config.
                let config = unsafe {
                    &*req
                        .port_details
                        .cast::<clap_sys::ext::ambisonic::clap_ambisonic_config>()
                };
                details = 2;
                req.channel_count == 1 && config.ordering == 1 && config.normalization == 1
            } else {
                false
            };
            if !valid {
                return None;
            }
        } else if !req.port_type.is_null() {
            // SAFETY: host guarantees valid C string matching `port_type`.
            let port_type = unsafe { CStr::from_ptr(req.port_type) };
            let matches_type = match req.channel_count {
                1 => port_type == CLAP_PORT_MONO,
                2 => port_type == CLAP_PORT_STEREO,
                _ => false,
            };
            if !matches_type {
                return None;
            }
        }
        let side = if req.is_input {
            &mut next_in
        } else {
            &mut next_out
        };
        let index = req.port_index as usize;
        if req.channel_count == 0 {
            if index + 1 == side.len() {
                side.pop();
            } else {
                return None;
            }
        } else if let Some(slot) = side.get_mut(index) {
            *slot = req.channel_count;
        } else if index == side.len() {
            side.push(req.channel_count);
        } else {
            return None;
        }
    }
    [0u8, 2, 3, 6, 9]
        .into_iter()
        .find(|&layout| {
            audio_ports::layout_ports(layout) == (next_in.as_slice(), next_out.as_slice())
        })
        .map(|layout| (layout, details))
}

unsafe extern "C" fn can_apply(
    plugin: *const clap_plugin,
    requests: *const clap_audio_port_configuration_request,
    request_count: u32,
) -> bool {
    // SAFETY: host passed valid request slice for the call.
    unsafe { resolve_requests(plugin, requests, request_count) }.is_some()
}

unsafe extern "C" fn apply(
    plugin: *const clap_plugin,
    requests: *const clap_audio_port_configuration_request,
    request_count: u32,
) -> bool {
    // SAFETY: host passed valid request slice for the call.
    let Some((layout, details)) = (unsafe { resolve_requests(plugin, requests, request_count) })
    else {
        return false;
    };
    if !audio_ports::config_select(layout, CLAP_INVALID_ID) {
        return false;
    }
    audio_ports::set_details(details);
    true
}
