//! Selected-device workgroup ownership, prepared before HAL starts callbacks.
use crate::{AudioError, OutputDevice, WorkgroupFault, hardware};
use echode_engine::{PluginWorkgroup, RetainedWorkgroup};
use objc2_core_audio::{kAudioDevicePropertyIOThreadOSWorkgroup, kAudioObjectPropertyScopeGlobal};
use std::sync::Arc;

fn period_ticks(frames: u32, rate: u32, numer: u32, denom: u32) -> Result<u32, WorkgroupFault> {
    if frames == 0 || rate == 0 || numer == 0 || denom == 0 {
        return Err(WorkgroupFault::Period);
    }
    echode_time::block_period_ticks(frames, u128::from(rate), numer, denom)
        .and_then(|ticks| u32::try_from(ticks).ok())
        .filter(|ticks| *ticks > 0)
        .ok_or(WorkgroupFault::Period)
}

// Workgroup periods are specified in this public Mach clock's units.
#[allow(deprecated)]
pub(crate) fn selected(device: &OutputDevice) -> Result<PluginWorkgroup, AudioError> {
    let fault = |fault| AudioError::Workgroup {
        device: device.id,
        fault,
    };
    let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
    // SAFETY: the out parameter is correctly sized; this runs on control only.
    if unsafe { libc::mach_timebase_info(&mut timebase) } != libc::KERN_SUCCESS {
        return Err(fault(WorkgroupFault::Timebase));
    }
    let ticks = period_ticks(
        device.buffer_frames,
        device.sample_rate,
        timebase.numer,
        timebase.denom,
    )
    .map_err(fault)?;
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
    query: impl FnOnce(u32) -> Result<*mut std::ffi::c_void, AudioError>,
) -> Result<PluginWorkgroup, AudioError> {
    let object = query(device).map_err(|error| AudioError::Workgroup {
        device,
        fault: WorkgroupFault::Query(Box::new(error)),
    })?;
    // SAFETY: HAL transfers one retained reference for this property.
    let owner = unsafe { RetainedWorkgroup::from_retained(object, ticks) }.map_err(|reason| {
        AudioError::Workgroup {
            device,
            fault: WorkgroupFault::Retain(reason),
        }
    })?;
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
            Err(AudioError::CoreAudio {
                call: crate::CoreAudioCall::Property,
                status: -50,
            })
        });
        let Err(error) = result else {
            panic!("a failed lookup gave a workgroup");
        };
        assert!(matches!(
            &error,
            AudioError::Workgroup {
                device: 77,
                fault: WorkgroupFault::Query(cause),
            } if matches!(**cause, AudioError::CoreAudio { status: -50, .. })
        ));
        assert_eq!(
            error.to_string(),
            "output device 77 workgroup: CoreAudio error -50"
        );
        assert!(matches!(
            selected_with(77, 32000, |_| Ok(std::ptr::null_mut())),
            Err(AudioError::Workgroup {
                device: 77,
                fault: WorkgroupFault::Retain(_),
            })
        ));
    }
}
