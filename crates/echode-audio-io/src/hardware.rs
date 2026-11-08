use block2::RcBlock;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use objc2_core_audio::*;
use objc2_core_audio_types::*;
use objc2_core_foundation::{CFRetained, CFString};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutputDevice {
    pub id: u32,
    pub uid: String,
    pub name: String,
    pub sample_rate: u32,
    pub buffer_frames: u32,
    /// Output frames HAL keeps ahead of the device's position. The output
    /// callback must be done that far before the block's output time.
    pub safety_frames: u32,
    pub channels: u32,
    #[serde(skip)]
    pub(crate) formats: Vec<(u32, AudioStreamBasicDescription)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Inventory {
    pub default_device: u32,
    pub devices: Vec<OutputDevice>,
}

pub(crate) fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn checked(status: i32) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("CoreAudio error {status}"))
    }
}

pub(crate) fn get<T: Copy>(object: u32, selector: u32, scope: u32) -> Result<T, String> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut property = address(selector, scope);
    let mut size = u32::try_from(size_of::<T>()).map_err(|_| "property size overflow")?;
    // SAFETY: typed out storage is correctly sized and aligned. Only SDK-defined
    // POD types and pointer-valued properties are read with this helper.
    checked(unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut property),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    })?;
    if size as usize != size_of::<T>() {
        return Err("CoreAudio property size changed".into());
    }
    Ok(unsafe { value.assume_init() })
}

pub(crate) fn list(object: u32, selector: u32, scope: u32) -> Result<Vec<u32>, String> {
    let mut property = address(selector, scope);
    // Device/stream lists can change between the size query and read.
    for _ in 0..3 {
        let mut size = 0;
        checked(unsafe {
            AudioObjectGetPropertyDataSize(
                object,
                NonNull::from(&mut property),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
            )
        })?;
        if size == 0 {
            return Ok(Vec::new());
        }
        if !(size as usize).is_multiple_of(size_of::<u32>()) {
            return Err("invalid CoreAudio list size".into());
        }
        let mut values = vec![0u32; size as usize / size_of::<u32>()];
        let Some(pointer) = NonNull::new(values.as_mut_ptr()) else {
            return Ok(Vec::new());
        };
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&mut property),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
                pointer.cast(),
            )
        };
        if status == kAudioHardwareBadPropertySizeError {
            continue;
        }
        checked(status)?;
        if size as usize > values.len() * size_of::<u32>() {
            return Err("CoreAudio list grew".into());
        }
        values.truncate(size as usize / size_of::<u32>());
        return Ok(values);
    }
    Err("CoreAudio device list kept changing".into())
}

fn string(object: u32, selector: u32) -> Result<String, String> {
    let pointer: *mut CFString = get(object, selector, kAudioObjectPropertyScopeGlobal)?;
    let pointer = NonNull::new(pointer).ok_or("CoreAudio returned an empty name")?;
    // HAL transfers a retained CFString for these properties; release it once
    // after copying the UTF-8 name/UID on the control thread.
    let value = unsafe { CFRetained::from_raw(pointer) };
    Ok(value.to_string())
}

pub fn inventory() -> Result<Inventory, String> {
    let default_device = get(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
    )?;
    let mut devices = Vec::new();
    for id in list(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    )? {
        if get::<u32>(
            id,
            kAudioDevicePropertyDeviceIsAlive,
            kAudioObjectPropertyScopeGlobal,
        )
        .unwrap_or(0)
            == 0
        {
            continue;
        }
        let streams = list(
            id,
            kAudioDevicePropertyStreams,
            kAudioObjectPropertyScopeOutput,
        )?;
        if streams.is_empty() {
            continue;
        }
        let mut channels = 0u32;
        let mut formats = Vec::with_capacity(streams.len());
        for stream in streams {
            let format: AudioStreamBasicDescription = get(
                stream,
                kAudioStreamPropertyVirtualFormat,
                kAudioObjectPropertyScopeGlobal,
            )?;
            channels = channels
                .checked_add(format.mChannelsPerFrame)
                .ok_or("channel count overflow")?;
            formats.push((stream, format));
        }
        if channels == 0 {
            continue;
        }
        let rate: f64 = get(
            id,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        )?;
        if !rate.is_finite() || rate < 1.0 || rate > f64::from(u32::MAX) || rate.fract() != 0.0 {
            return Err("CoreAudio sample rate must be a positive whole number".into());
        }
        let buffer_frames = get(
            id,
            kAudioDevicePropertyBufferFrameSize,
            kAudioObjectPropertyScopeGlobal,
        )?;
        if buffer_frames == 0 {
            continue;
        }
        // Every HAL device reports this. A failed read counts as no offset:
        // the callback still ends within one block of entry (ADR 0283).
        let safety_frames = get(
            id,
            kAudioDevicePropertySafetyOffset,
            kAudioObjectPropertyScopeOutput,
        )
        .unwrap_or(0);
        devices.push(OutputDevice {
            id,
            uid: string(id, kAudioDevicePropertyDeviceUID)?,
            name: string(id, kAudioObjectPropertyName)?,
            sample_rate: rate as u32,
            buffer_frames,
            safety_frames,
            channels,
            formats,
        });
    }
    Ok(Inventory {
        default_device,
        devices,
    })
}

pub(crate) fn validate_streams(device: &OutputDevice) -> Result<(), String> {
    for stream in list(
        device.id,
        kAudioDevicePropertyStreams,
        kAudioObjectPropertyScopeOutput,
    )? {
        let format: AudioStreamBasicDescription = get(
            stream,
            kAudioStreamPropertyVirtualFormat,
            kAudioObjectPropertyScopeGlobal,
        )?;
        let channels = if format.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0 {
            1
        } else {
            format.mChannelsPerFrame
        };
        if format.mFormatID != kAudioFormatLinearPCM
            || format.mBitsPerChannel != 32
            || format.mFormatFlags & kAudioFormatFlagIsFloat == 0
            || format.mFormatFlags & kAudioFormatFlagIsBigEndian != 0
            || format.mBytesPerFrame != channels.saturating_mul(4)
            || format.mSampleRate != f64::from(device.sample_rate)
        {
            return Err("the output device must provide native 32-bit float PCM".into());
        }
    }
    Ok(())
}

/// HAL copies the block and retains its captured epoch, including notifications
/// already dispatched when a listener is removed.
pub(crate) struct Notifications {
    epoch: Arc<AtomicU64>,
    block: RcBlock<dyn Fn(u32, NonNull<AudioObjectPropertyAddress>)>,
    registered: Vec<(u32, AudioObjectPropertyAddress)>,
}

impl Notifications {
    pub(crate) fn workgroup(device: u32) -> Result<Self, String> {
        let epoch = Arc::new(AtomicU64::new(0));
        let captured = Arc::clone(&epoch);
        let block = RcBlock::new(move |_: u32, _: NonNull<AudioObjectPropertyAddress>| {
            captured.fetch_add(1, Ordering::Release);
        });
        let mut value = Self {
            epoch,
            block,
            registered: Vec::new(),
        };
        value.add(device, kAudioDevicePropertyIOThreadOSWorkgroup)?;
        Ok(value)
    }

    pub(crate) fn changed(&self) -> bool {
        self.epoch.load(Ordering::Acquire) != 0
    }

    pub(crate) fn new(inventory: &Inventory, epoch: Arc<AtomicU64>) -> Result<Self, String> {
        let captured = Arc::clone(&epoch);
        let block = RcBlock::new(move |_: u32, _: NonNull<AudioObjectPropertyAddress>| {
            captured.fetch_add(1, Ordering::Release);
        });
        let mut value = Self {
            epoch,
            block,
            registered: Vec::new(),
        };
        value.add(
            kAudioObjectSystemObject as u32,
            kAudioHardwarePropertyDevices,
        )?;
        value.add(
            kAudioObjectSystemObject as u32,
            kAudioHardwarePropertyDefaultOutputDevice,
        )?;
        value.add(
            kAudioObjectSystemObject as u32,
            kAudioHardwarePropertyServiceRestarted,
        )?;
        for device in &inventory.devices {
            for selector in [
                kAudioDevicePropertyNominalSampleRate,
                kAudioDevicePropertyBufferFrameSize,
                kAudioDevicePropertyDeviceIsAlive,
            ] {
                value.add(device.id, selector)?;
            }
            value.add_scoped(
                device.id,
                kAudioDevicePropertyStreams,
                kAudioObjectPropertyScopeOutput,
            )?;
            // Like its read, an offset without a listener never blocks output;
            // a missed change keeps the old offset until the next reopen.
            let _ = value.add_scoped(
                device.id,
                kAudioDevicePropertySafetyOffset,
                kAudioObjectPropertyScopeOutput,
            );
            for (stream, _) in &device.formats {
                value.add(*stream, kAudioStreamPropertyVirtualFormat)?;
            }
        }
        Ok(value)
    }
    fn add(&mut self, id: u32, selector: u32) -> Result<(), String> {
        self.add_scoped(id, selector, kAudioObjectPropertyScopeGlobal)
    }
    fn add_scoped(&mut self, id: u32, selector: u32, scope: u32) -> Result<(), String> {
        let mut property = address(selector, scope);
        let pointer = NonNull::from(&*self.block).as_ptr();
        checked(unsafe {
            AudioObjectAddPropertyListenerBlock(id, NonNull::from(&mut property), None, pointer)
        })?;
        self.registered.push((id, property));
        Ok(())
    }
    pub(crate) fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::Release);
    }
}

impl Drop for Notifications {
    fn drop(&mut self) {
        let pointer = NonNull::from(&*self.block).as_ptr();
        for (id, property) in &mut self.registered {
            let _ = unsafe {
                AudioObjectRemovePropertyListenerBlock(*id, NonNull::from(property), None, pointer)
            };
        }
    }
}
