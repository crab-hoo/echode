//! Selected-device workgroup ownership, prepared before HAL starts callbacks.
use crate::{OutputDevice, hardware};
use echode_engine::{PluginWorkgroup, RetainedWorkgroup};
use objc2_core_audio::{kAudioDevicePropertyIOThreadOSWorkgroup, kAudioObjectPropertyScopeGlobal};
use std::sync::Arc;

fn period_ticks(frames: u32, rate: u32, numer: u32, denom: u32) -> Result<u32, String> {
    if frames == 0 || rate == 0 || numer == 0 || denom == 0 {
        return Err("invalid device workgroup period".into());
    }
    let ticks = echode_time::block_period_ticks(frames, u128::from(rate), numer, denom)
        .ok_or_else(|| "device workgroup period is out of range".to_string())?;
    u32::try_from(ticks)
        .ok()
        .filter(|ticks| *ticks > 0)
        .ok_or_else(|| "device workgroup period is out of range".into())
}

// Workgroup periods are specified in this public Mach clock's units.
#[allow(deprecated)]
pub(crate) fn selected(device: &OutputDevice) -> Result<PluginWorkgroup, String> {
    let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
    // SAFETY: the out parameter is correctly sized; this runs on control only.
    if unsafe { libc::mach_timebase_info(&mut timebase) } != libc::KERN_SUCCESS {
        return Err("could not read the device workgroup timebase".into());
    }
    let ticks = period_ticks(
        device.buffer_frames,
        device.sample_rate,
        timebase.numer,
        timebase.denom,
    )?;
    selected_with(device.id, ticks, |id| {
        hardware::get(
            id,
            kAudioDevicePropertyIOThreadOSWorkgroup,
            kAudioObjectPropertyScopeGlobal,
        )
    })
}

fn selected_with(
    device: u32,
    ticks: u32,
    query: impl FnOnce(u32) -> Result<*mut std::ffi::c_void, String>,
) -> Result<PluginWorkgroup, String> {
    let object =
        query(device).map_err(|error| format!("output device {device} workgroup: {error}"))?;
    // SAFETY: HAL transfers one retained reference for this property.
    let owner = unsafe { RetainedWorkgroup::from_retained(object, ticks) }
        .map_err(|error| format!("output device {device} workgroup: {error}"))?;
    Ok(Arc::new(move || owner.handoff()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_device_and_period_are_not_the_system_default() {
        assert_eq!(period_ticks(64, 48000, 125, 3).unwrap(), 32000);
        assert_eq!(period_ticks(128, 96000, 125, 3).unwrap(), 32000);
        assert!(period_ticks(0, 48000, 125, 3).is_err());
        assert!(period_ticks(64, 0, 125, 3).is_err());
        assert!(period_ticks(64, 48000, 0, 3).is_err());
        let result = selected_with(77, 32000, |device| {
            assert_eq!(device, 77);
            Err("fixture lookup failed".into())
        });
        assert!(
            matches!(result, Err(ref reason) if reason.contains("77") && reason.contains("fixture lookup failed"))
        );
        assert!(selected_with(77, 32000, |_| Ok(std::ptr::null_mut())).is_err());
    }
}
