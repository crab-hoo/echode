use super::*;
use clap_sys::ext::audio_ports::clap_audio_port_info;
use std::cell::Cell;
use std::ffi::{c_char, c_void};

struct Fixture {
    ports: clap_plugin_audio_ports_config,
    mode: u8,
    info: Option<clap_plugin_audio_ports_config_info>,
    info_compat: bool,
    info_mode: u8,
    configurable: Option<clap_plugin_configurable_audio_ports>,
    configurable_compat: bool,
    configurable_accept: bool,
    configurable_seen: Cell<u8>,
}

impl Fixture {
    fn new(mode: u8) -> Self {
        Self {
            ports: clap_plugin_audio_ports_config {
                count: Some(count),
                get: Some(get),
                select: Some(choose),
            },
            mode,
            info: None,
            info_compat: false,
            info_mode: 0,
            configurable: None,
            configurable_compat: false,
            configurable_accept: true,
            configurable_seen: Cell::new(0),
        }
    }
    fn with_info(mut self, compat: bool, info_mode: u8) -> Self {
        self.mode = 10;
        self.info_compat = compat;
        self.info_mode = info_mode;
        self.info = Some(clap_plugin_audio_ports_config_info {
            current_config: Some(info_current),
            get: Some(info_get),
        });
        self
    }
    fn with_configurable(mut self, compat: bool, accept: bool) -> Self {
        self.configurable_compat = compat;
        self.configurable_accept = accept;
        self.configurable = Some(clap_plugin_configurable_audio_ports {
            can_apply_configuration: Some(conf_can_apply),
            apply_configuration: Some(conf_apply),
        });
        self
    }
    fn plugin(&self) -> clap_plugin {
        // SAFETY: clap_plugin consists of raw pointers and optional callbacks.
        let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
        plugin.plugin_data = std::ptr::from_ref(self).cast_mut().cast();
        plugin.get_extension = Some(extension);
        plugin
    }
}

unsafe fn fixture<'a>(plugin: *const clap_plugin) -> &'a Fixture {
    // SAFETY: all these callbacks are called with the Fixture's local plugin,
    // and Fixture outlives every callback and borrowed extension table.
    unsafe { &*(*plugin).plugin_data.cast::<Fixture>() }
}
unsafe extern "C" fn extension(plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
    // SAFETY: the caller passes a terminated extension id and the local plugin.
    unsafe {
        let state = fixture(plugin);
        let id = CStr::from_ptr(id);
        let info_id = if state.info_compat {
            CLAP_EXT_AUDIO_PORTS_CONFIG_INFO_COMPAT
        } else {
            CLAP_EXT_AUDIO_PORTS_CONFIG_INFO
        };
        let configurable_id = if state.configurable_compat {
            CLAP_EXT_CONFIGURABLE_AUDIO_PORTS_COMPAT
        } else {
            CLAP_EXT_CONFIGURABLE_AUDIO_PORTS
        };
        if id == CLAP_EXT_AUDIO_PORTS_CONFIG {
            std::ptr::from_ref(&state.ports).cast()
        } else if id == info_id {
            state
                .info
                .as_ref()
                .map_or(std::ptr::null(), |table| std::ptr::from_ref(table).cast())
        } else if id == configurable_id {
            state
                .configurable
                .as_ref()
                .map_or(std::ptr::null(), |table| std::ptr::from_ref(table).cast())
        } else {
            std::ptr::null()
        }
    }
}
unsafe extern "C" fn count(plugin: *const clap_plugin) -> u32 {
    // SAFETY: the local plugin belongs to its Fixture.
    if unsafe { fixture(plugin) }.mode == 0 {
        0
    } else {
        2
    }
}
unsafe extern "C" fn get(
    plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_audio_ports_config,
) -> bool {
    // SAFETY: the local plugin belongs to its Fixture; read() passes writable
    // storage for one descriptor. Zero is valid for all descriptor fields.
    unsafe {
        let mode = fixture(plugin).mode;
        if mode == 2 {
            return false;
        }
        *info = std::mem::zeroed();
        (*info).id = match mode {
            3 => u32::MAX,
            4 => 100,
            _ => index + 100,
        };
        (*info).name.fill(-1 as c_char); // No NUL and invalid UTF-8 is bounded.
        if mode == 10 {
            (*info).has_main_input = true;
            (*info).input_port_count = if index == 0 { 1 } else { 2 };
            (*info).main_input_channel_count = 1;
            (*info).has_main_output = true;
            (*info).output_port_count = 1;
            (*info).main_output_channel_count = 2;
        } else {
            (*info).has_main_input = mode == 5;
            (*info).has_main_output = mode == 6;
            // With no main ports, counts and channel hints are not dereferenced or
            // interpreted as exact bus layouts.
            (*info).main_input_channel_count = u32::MAX;
            (*info).main_output_channel_count = u32::MAX;
        }
        true
    }
}
unsafe extern "C" fn choose(_plugin: *const clap_plugin, id: u32) -> bool {
    id == 100
}
unsafe extern "C" fn info_current(plugin: *const clap_plugin) -> u32 {
    // SAFETY: the local plugin belongs to its Fixture.
    match unsafe { fixture(plugin) }.info_mode {
        0 => 101,
        1 => CLAP_INVALID_ID,
        2 => 999,
        _ => 100,
    }
}
unsafe extern "C" fn info_get(
    plugin: *const clap_plugin,
    config_id: u32,
    port_index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    // SAFETY: the local plugin belongs to its Fixture and `info` is writable.
    let mode = unsafe { fixture(plugin) }.info_mode;
    if mode == 3 {
        return false;
    }
    let channels = if mode == 4 {
        0
    } else if mode == 5 && config_id == 100 && is_input && port_index == 0 {
        2
    } else if mode == 6 && config_id == 100 && !is_input && port_index == 0 {
        1
    } else if is_input {
        match (config_id, port_index) {
            (100, 0) | (101, 0) | (101, 1) => 1,
            _ => return false,
        }
    } else if matches!((config_id, port_index), (100, 0) | (101, 0)) {
        2
    } else {
        return false;
    };
    // SAFETY: `info` points to storage for one `clap_audio_port_info`.
    unsafe {
        *info = std::mem::zeroed();
        (*info).channel_count = channels;
    }
    true
}
unsafe fn check_requests(
    plugin: *const clap_plugin,
    requests: *const clap_audio_port_configuration_request,
    request_count: u32,
    bit: u8,
) -> bool {
    // SAFETY: the local plugin belongs to its Fixture.
    let state = unsafe { fixture(plugin) };
    if request_count == 0 {
        if requests.is_null() {
            state
                .configurable_seen
                .set(state.configurable_seen.get() | bit);
        }
        return requests.is_null() && state.configurable_accept;
    }
    if requests.is_null() {
        return false;
    }
    // SAFETY: `requests` contains `request_count` initialized requests from the host.
    let slice = unsafe { std::slice::from_raw_parts(requests, request_count as usize) };
    let valid_types = slice.iter().all(|req| {
        if !req.port_details.is_null() {
            // SAFETY: the host lends the static type string for the callback.
            let kind = unsafe { CStr::from_ptr(req.port_type) };
            if kind == CLAP_PORT_SURROUND {
                // SAFETY: the host must lend exactly channel_count readable bytes.
                let map = unsafe {
                    std::slice::from_raw_parts(
                        req.port_details.cast::<u8>(),
                        req.channel_count as usize,
                    )
                };
                return map == [5, 0, 2];
            }
            if kind == CLAP_PORT_AMBISONIC {
                // SAFETY: this type uses one aligned clap_ambisonic_config.
                let config = unsafe { &*req.port_details.cast::<clap_ambisonic_config>() };
                return req.channel_count == 4 && config.ordering == 1 && config.normalization == 2;
            }
            return false;
        }
        match req.channel_count {
            1 => {
                !req.port_type.is_null()
                    // SAFETY: host guarantees valid NUL-terminated CLAP_PORT_MONO.
                    && unsafe { CStr::from_ptr(req.port_type) } == CLAP_PORT_MONO
            }
            2 => {
                !req.port_type.is_null()
                    // SAFETY: host guarantees valid NUL-terminated CLAP_PORT_STEREO.
                    && unsafe { CStr::from_ptr(req.port_type) } == CLAP_PORT_STEREO
            }
            _ => req.port_type.is_null(),
        }
    });
    if valid_types {
        state
            .configurable_seen
            .set(state.configurable_seen.get() | bit);
    }
    valid_types && state.configurable_accept
}
unsafe extern "C" fn conf_can_apply(
    plugin: *const clap_plugin,
    requests: *const clap_audio_port_configuration_request,
    request_count: u32,
) -> bool {
    // SAFETY: delegates on main with the local fixture's requests pointer.
    unsafe { check_requests(plugin, requests, request_count, 0b01) }
}
unsafe extern "C" fn conf_apply(
    plugin: *const clap_plugin,
    requests: *const clap_audio_port_configuration_request,
    request_count: u32,
) -> bool {
    // SAFETY: delegates on main with the local fixture's requests pointer.
    unsafe { check_requests(plugin, requests, request_count, 0b10) }
}

#[test]
fn audio_configuration_descriptions_keep_ids_and_bound_optional_metadata() {
    assert!(read(&Fixture::new(0).plugin()).unwrap().configs.is_empty());
    let configs = read(&Fixture::new(1).plugin()).unwrap().configs;
    assert_eq!(configs.iter().map(|c| c.id).collect::<Vec<_>>(), [100, 101]);
    assert!(configs.iter().all(|c| c.name.chars().count()
        == clap_sys::string_sizes::CLAP_NAME_SIZE
        && c.main_input_channels == 0
        && c.main_output_channels == 0));
    for mode in 2..=6 {
        assert_eq!(
            read(&Fixture::new(mode).plugin()),
            Err(PluginFailure::AudioConfigUndescribed)
        );
    }
}

#[test]
fn missing_callbacks_and_refused_selections_have_distinct_causes() {
    let mut fixture = Fixture::new(1);
    fixture.ports.count = None;
    assert_eq!(
        read(&fixture.plugin()),
        Err(PluginFailure::NoAudioConfigCount)
    );
    fixture.ports.count = Some(count);
    fixture.ports.get = None;
    assert_eq!(
        read(&fixture.plugin()),
        Err(PluginFailure::NoAudioConfigInfo)
    );
    fixture.ports.select = None;
    assert_eq!(
        select(&fixture.plugin(), 100),
        Err(PluginFailure::NoAudioConfigSelect)
    );
    fixture.ports.select = Some(choose);
    assert_eq!(select(&fixture.plugin(), 100), Ok(()));
    for id in [0, 1, u32::MAX] {
        assert_eq!(
            select(&fixture.plugin(), id),
            Err(PluginFailure::AudioConfigRefused)
        );
    }
}

#[test]
fn audio_ports_config_info_reads_current_and_ports_across_v1_and_compat() {
    for compat in [false, true] {
        let with_current = read(&Fixture::new(10).with_info(compat, 0).plugin()).unwrap();
        assert_eq!(with_current.current_id, Some(101));
        assert_eq!(
            with_current
                .configs
                .iter()
                .map(|c| &c.ports)
                .collect::<Vec<_>>(),
            [
                &Some(PortLayout::new(vec![1], vec![2])),
                &Some(PortLayout::new(vec![1, 1], vec![2])),
            ]
        );
        let unset_current = read(&Fixture::new(10).with_info(compat, 1).plugin()).unwrap();
        assert_eq!(unset_current.current_id, None);
        assert!(unset_current.configs.iter().all(|c| c.ports.is_some()));
    }
}

#[test]
fn audio_ports_config_info_rejects_missing_callbacks_and_inconsistent_layouts() {
    let mut missing_current = Fixture::new(10).with_info(false, 0);
    missing_current.info.as_mut().unwrap().current_config = None;
    assert_eq!(
        read(&missing_current.plugin()),
        Err(PluginFailure::NoAudioConfigCurrent)
    );
    let mut missing_get = Fixture::new(10).with_info(false, 0);
    missing_get.info.as_mut().unwrap().get = None;
    assert_eq!(
        read(&missing_get.plugin()),
        Err(PluginFailure::NoAudioConfigPortInfo)
    );
    for bad_mode in [2, 3, 4, 5, 6] {
        assert_eq!(
            read(&Fixture::new(10).with_info(false, bad_mode).plugin()),
            Err(PluginFailure::AudioConfigUndescribed)
        );
    }
}

#[test]
fn configurable_audio_ports_queries_and_applies_across_v1_and_compat() {
    let requests = [
        AudioPortRequest {
            input: true,
            index: 1,
            channels: 0,
            details: None,
        },
        AudioPortRequest {
            input: true,
            index: 0,
            channels: 1,
            details: None,
        },
        AudioPortRequest {
            input: false,
            index: 0,
            channels: 2,
            details: None,
        },
        AudioPortRequest {
            input: false,
            index: 1,
            channels: 4,
            details: None,
        },
        AudioPortRequest {
            input: true,
            index: 2,
            channels: 3,
            details: Some(AudioPortDetails::Surround(vec![5, 0, 2])),
        },
        AudioPortRequest {
            input: false,
            index: 2,
            channels: 4,
            details: Some(AudioPortDetails::Ambisonic {
                ordering: 1,
                normalization: 2,
            }),
        },
    ];
    let missing = Fixture::new(0);
    assert_eq!(
        can_apply(&missing.plugin(), &requests),
        Err(PluginFailure::NoConfigurableAudioPorts)
    );
    assert_eq!(
        apply(&missing.plugin(), &requests),
        Err(PluginFailure::NoConfigurableAudioPorts)
    );
    let mut missing_can_apply = Fixture::new(0).with_configurable(false, true);
    missing_can_apply
        .configurable
        .as_mut()
        .unwrap()
        .can_apply_configuration = None;
    assert_eq!(
        can_apply(&missing_can_apply.plugin(), &requests),
        Err(PluginFailure::NoConfigurableAudioPortsCanApply)
    );
    let mut missing_apply = Fixture::new(0).with_configurable(false, true);
    missing_apply
        .configurable
        .as_mut()
        .unwrap()
        .apply_configuration = None;
    assert_eq!(
        apply(&missing_apply.plugin(), &requests),
        Err(PluginFailure::NoConfigurableAudioPortsApply)
    );
    for compat in [false, true] {
        let accept = Fixture::new(0).with_configurable(compat, true);
        assert_eq!(can_apply(&accept.plugin(), &requests), Ok(true));
        assert_eq!(apply(&accept.plugin(), &requests), Ok(()));
        assert_eq!(can_apply(&accept.plugin(), &[]), Ok(true));
        assert_eq!(apply(&accept.plugin(), &[]), Ok(()));
        assert_eq!(accept.configurable_seen.get(), 0b11);
        let refuse = Fixture::new(0).with_configurable(compat, false);
        assert_eq!(can_apply(&refuse.plugin(), &requests), Ok(false));
        assert_eq!(
            apply(&refuse.plugin(), &requests),
            Err(PluginFailure::ConfigurableAudioPortsRefused)
        );
        assert_eq!(refuse.configurable_seen.get(), 0b11);
    }
}

#[test]
fn invalid_surround_lengths_never_call_the_plugin() {
    for (channels, map) in [(0, vec![]), (1, vec![]), (2, vec![0]), (1, vec![0, 1])] {
        let fixture = Fixture::new(0).with_configurable(false, true);
        let requests = [AudioPortRequest {
            input: false,
            index: 0,
            channels,
            details: Some(AudioPortDetails::Surround(map)),
        }];
        assert_eq!(
            can_apply(&fixture.plugin(), &requests),
            Err(PluginFailure::ConfigurableAudioPortsRefused)
        );
        assert_eq!(
            apply(&fixture.plugin(), &requests),
            Err(PluginFailure::ConfigurableAudioPortsRefused)
        );
        assert_eq!(
            fixture.configurable_seen.get(),
            0,
            "invalid map must not cross the ABI"
        );
    }
}
