use super::*;
use std::ffi::{c_char, c_void};

struct Fixture {
    ports: clap_plugin_audio_ports_config,
    mode: u8,
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
        }
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
        if CStr::from_ptr(id) == CLAP_EXT_AUDIO_PORTS_CONFIG {
            std::ptr::from_ref(&fixture(plugin).ports).cast()
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
        (*info).has_main_input = mode == 5;
        (*info).has_main_output = mode == 6;
        // With no main ports, counts and channel hints are not dereferenced or
        // interpreted as exact bus layouts.
        (*info).main_input_channel_count = u32::MAX;
        (*info).main_output_channel_count = u32::MAX;
        true
    }
}
unsafe extern "C" fn choose(_plugin: *const clap_plugin, id: u32) -> bool {
    id == 100
}

#[test]
fn audio_configuration_descriptions_keep_ids_and_bound_optional_metadata() {
    assert!(read(&Fixture::new(0).plugin()).unwrap().is_empty());
    let configs = read(&Fixture::new(1).plugin()).unwrap();
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
