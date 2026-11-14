//! Sends a description rescan from get_info while the host filters a value relay.
use crate::{Fault, TestGain, fault_now};
use clack_plugin::extensions::prelude::PluginWrapper;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_value,
    clap_output_events,
};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, CLAP_PARAM_RESCAN_INFO, clap_host_params};
use clap_sys::plugin::clap_plugin;
use std::sync::atomic::{AtomicBool, Ordering};

static ARMED: AtomicBool = AtomicBool::new(false);

/// # Safety
/// The plugin is live on its main thread for this get_info call.
pub(crate) unsafe fn on_info(plugin: *const clap_plugin, index: u32) {
    if fault_now() != Fault::ParamRescan || index != 0 || !ARMED.swap(false, Ordering::AcqRel) {
        return;
    }
    // SAFETY: the caller supplies this live plugin on its main thread. The host
    // extension table and pointer remain valid throughout the notification.
    unsafe {
        PluginWrapper::<TestGain>::handle(plugin, |wrapper| {
            let host = wrapper.main_thread().host.shared().as_raw();
            if let Some(get) = host.get_extension {
                let table = get(host, CLAP_EXT_PARAMS.as_ptr()).cast::<clap_host_params>();
                if let Some(rescan) = table.as_ref().and_then(|table| table.rescan) {
                    rescan(host, CLAP_PARAM_RESCAN_INFO);
                }
            }
            Ok(())
        });
    }
}

/// # Safety
/// The output list is null or remains valid for this flush call.
pub(crate) unsafe fn on_flush(output: *const clap_output_events, value: f64) {
    if fault_now() != Fault::ParamRescan {
        return;
    }
    // SAFETY: the caller provides null or its live output list.
    let Some(output) = (unsafe { output.as_ref() }) else {
        return;
    };
    let Some(push) = output.try_push else {
        return;
    };
    let event = clap_event_param_value {
        header: clap_event_header {
            size: size_of::<clap_event_param_value>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: crate::params::GAIN,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    };
    ARMED.store(true, Ordering::Release);
    // SAFETY: the host copies the stack event before returning.
    unsafe { push(output, &event.header) };
}
