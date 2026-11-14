//! Exercises the host log extension without allocating on the audio thread.
use clack_plugin::extensions::prelude::*;
use clack_plugin::prelude::PluginError;
use clap_sys::ext::log::*;
use std::ffi::CStr;

pub(crate) fn initialize(host: HostSharedHandle<'_>) -> Result<(), PluginError> {
    for severity in CLAP_LOG_DEBUG..=CLAP_LOG_PLUGIN_MISBEHAVING {
        send(host, severity, c"main log probe")?;
    }
    send(host, 99, c"unknown severity probe")?;
    send(host, CLAP_LOG_INFO, c"invalid UTF-8: \xff")?;
    // This oversized diagnostic is a fixture, not a plugin or channel limit.
    let mut long = [b'x'; 1301];
    long[1300] = 0;
    let long = CStr::from_bytes_with_nul(&long).unwrap();
    send(host, CLAP_LOG_WARNING, long)?;
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(move || send(host, CLAP_LOG_INFO, c"background log probe").is_ok())
            })
            .collect();
        for thread in threads {
            if !thread.join().unwrap() {
                return Err(PluginError::Message("clap.log background probe failed"));
            }
        }
        Ok(())
    })?;
    if std::env::var("ECHODE_TEST_FAULT").as_deref() == Ok("host-log-refuse") {
        return Err(PluginError::Message("deliberate refusal after logging"));
    }
    Ok(())
}

pub(crate) fn process(host: HostSharedHandle<'_>) -> Result<(), PluginError> {
    send(host, CLAP_LOG_ERROR, c"audio log probe\nsecond line: 音")
}

fn send(
    host: HostSharedHandle<'_>,
    severity: clap_log_severity,
    message: &CStr,
) -> Result<(), PluginError> {
    let Some(extension) = host.get_extension::<HostLog>() else {
        return Err(PluginError::Message("clap.log extension is unavailable"));
    };
    // SAFETY: the extension belongs to this live host. CLAP permits logging
    // from any thread, and the message stays valid for the whole call.
    unsafe {
        let table = extension.0.as_ptr().as_ref();
        let Some(log) = table.log else {
            return Err(PluginError::Message("clap.log callback is unavailable"));
        };
        log(extension.0.host_ptr().as_ptr(), severity, message.as_ptr());
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct HostLog(RawExtension<HostExtensionSide, clap_host_log>);
// SAFETY: this identifier names the CLAP host log extension.
unsafe impl Extension for HostLog {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_LOG];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: the identifier selects the `clap_host_log` table.
        Self(unsafe { raw.cast() })
    }
}
