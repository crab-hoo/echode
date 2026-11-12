//! Mixer parameters and CLAP devices recorded on a track.
//!
//! A track ends with one mixer. CLAP devices sit before it, in insertion order.
//! Their process and saved state are not stored here.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::device_fields::device_fields;
use crate::id::Id;
use crate::model::ModelError;
use crate::parameter_fields::parameter_fields;

/// Linear amplitude of about +12 dB, the fader's upper end.
pub const GAIN_MAX: f32 = 4.0;

/// Normalized balance pan endpoints.
pub const PAN_MIN: f32 = -1.0;
pub const PAN_MAX: f32 = 1.0;

/// Ids created with a track so a journal can rebuild the same mixer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixerIds {
    pub device: Id,
    pub gain: Id,
    pub pan: Id,
    pub mute: Id,
    pub solo: Id,
}

impl MixerIds {
    pub fn generate() -> Self {
        Self {
            device: Id::generate(),
            gain: Id::generate(),
            pan: Id::generate(),
            mute: Id::generate(),
            solo: Id::generate(),
        }
    }

    /// Five ids at `base` through `base + 4`. Tests use a base that does not
    /// collide with the other ids in that project.
    pub const fn from_base(base: u128) -> Self {
        Self {
            device: Id::from_raw(base),
            gain: Id::from_raw(base + 1),
            pan: Id::from_raw(base + 2),
            mute: Id::from_raw(base + 3),
            solo: Id::from_raw(base + 4),
        }
    }
}

/// What a device on a track is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Mixer,
    Clap,
}

impl DeviceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mixer => "mixer",
            Self::Clap => "clap",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "mixer" => Some(Self::Mixer),
            "clap" => Some(Self::Clap),
            _ => None,
        }
    }
}

impl Serialize for DeviceKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DeviceKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| D::Error::custom(format!("unknown device kind {text}")))
    }
}

/// A CLAP id the host can look up. Empty text and control characters are refused.
pub fn valid_plugin_id(plugin: &str) -> bool {
    !plugin.is_empty() && plugin.chars().all(|ch| !ch.is_control())
}

/// The object store's name for `bytes`: BLAKE3-256 as 64 lowercase hex digits.
/// Saved plugin states and imported PCM are named this way, so two equal
/// names are the same bytes.
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// BLAKE3-256 as 64 lowercase hex digits, the only plugin-state spelling.
pub fn valid_state_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Which mixer control a parameter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKey {
    Gain,
    Pan,
    Mute,
    Solo,
}

impl ParamKey {
    pub const ORDER: [Self; 4] = [Self::Gain, Self::Pan, Self::Mute, Self::Solo];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gain => "gain",
            Self::Pan => "pan",
            Self::Mute => "mute",
            Self::Solo => "solo",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "gain" => Some(Self::Gain),
            "pan" => Some(Self::Pan),
            "mute" => Some(Self::Mute),
            "solo" => Some(Self::Solo),
            _ => None,
        }
    }

    pub fn allows(self, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        match self {
            Self::Gain => (0.0..=GAIN_MAX).contains(&value),
            Self::Pan => (PAN_MIN..=PAN_MAX).contains(&value),
            Self::Mute | Self::Solo => value == 0.0 || value == 1.0,
        }
    }

    pub(crate) fn ends(self) -> (f32, f32, f32) {
        match self {
            Self::Gain => (0.0, GAIN_MAX, 1.0),
            Self::Pan => (PAN_MIN, PAN_MAX, 0.0),
            Self::Mute | Self::Solo => (0.0, 1.0, 0.0),
        }
    }
}

impl Serialize for ParamKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ParamKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| D::Error::custom(format!("unknown parameter {text}")))
    }
}

macro_rules! define_parameter {
    ($($property:ident => $field:ident),+ $(,)?) => {
        /// One addressable value. Equality uses float bits.
        ///
        /// Serde checks the record's shape, not mixer invariants. Validate the
        /// assembled project before publishing a decoded parameter.
        #[derive(Clone, Debug, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct Parameter {
            pub id: Id,
            pub key: ParamKey,
            $(pub $field: f32,)+
        }

        impl PartialEq for Parameter {
            fn eq(&self, other: &Self) -> bool {
                self.id == other.id
                    && self.key == other.key
                    $(&& self.$field.to_bits() == other.$field.to_bits())+
            }
        }

        impl Eq for Parameter {}
    };
}

parameter_fields!(define_parameter);

impl Parameter {
    fn at_default(id: Id, key: ParamKey) -> Self {
        let (min, max, default) = key.ends();
        Self {
            id,
            key,
            min,
            max,
            default,
            value: default,
        }
    }
}

macro_rules! define_device {
    (
        fields { $(
            $(#[$meta:meta])*
            $property:ident => $field:ident: $type:ty,
            $encode:ident, $read:ident, $parse:ident; $ffi:ident $getter:ident;
        )+ }
        create [$($create:ident),+];
        diff [$($diff:ident),+];
    ) => {
        /// A device on a track. The mixer keeps [`ParamKey::ORDER`]. A CLAP device
        /// has no model parameters. Serde checks shape, not project invariants or
        /// the existence and integrity of a saved plugin-state object.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct Device {
            pub id: Id,
            pub kind: DeviceKind,
            $($(#[$meta])* pub $field: $type,)+
            pub parameters: Vec<Parameter>,
        }
    };
}

device_fields!(define_device);

impl Device {
    pub fn unity(ids: MixerIds) -> Self {
        Self {
            id: ids.device,
            kind: DeviceKind::Mixer,
            plugin: None,
            state: None,
            bypassed: false,
            offline: false,
            pinned: Vec::new(),
            parameters: vec![
                Parameter::at_default(ids.gain, ParamKey::Gain),
                Parameter::at_default(ids.pan, ParamKey::Pan),
                Parameter::at_default(ids.mute, ParamKey::Mute),
                Parameter::at_default(ids.solo, ParamKey::Solo),
            ],
        }
    }

    pub fn clap(id: Id, plugin: String) -> Self {
        Self {
            id,
            kind: DeviceKind::Clap,
            plugin: Some(plugin),
            state: None,
            bypassed: false,
            offline: false,
            pinned: Vec::new(),
            parameters: Vec::new(),
        }
    }

    pub fn value(&self, key: ParamKey) -> Option<f32> {
        self.parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .map(|parameter| parameter.value)
    }
}

/// A mixer with the four default parameters, for tests that build a track by hand.
pub fn test_mixer(track: u128) -> Device {
    Device::unity(MixerIds::from_base(
        track.saturating_mul(16).saturating_add(0xA000),
    ))
}

pub(crate) fn validate_devices(
    devices: &[Device],
    seen: &mut std::collections::HashSet<Id>,
) -> Result<(), ModelError> {
    let Some(last) = devices.len().checked_sub(1) else {
        return Err(ModelError::InvalidDevice);
    };
    for (index, device) in devices.iter().enumerate() {
        if index == last {
            if device.plugin.is_some() {
                return Err(ModelError::InvalidDevice);
            }
            validate_mixer(device, seen)?;
        } else {
            validate_clap(device, seen)?;
        }
    }
    Ok(())
}

fn validate_clap(
    device: &Device,
    seen: &mut std::collections::HashSet<Id>,
) -> Result<(), ModelError> {
    if !seen.insert(device.id) {
        return Err(ModelError::DuplicateId);
    }
    let Some(plugin) = device.plugin.as_deref() else {
        return Err(ModelError::InvalidDevice);
    };
    if device.kind != DeviceKind::Clap || !device.parameters.is_empty() || !valid_plugin_id(plugin)
    {
        return Err(ModelError::InvalidDevice);
    }
    if device
        .state
        .as_deref()
        .is_some_and(|hash| !valid_state_hash(hash))
        || !valid_pins(&device.pinned)
    {
        return Err(ModelError::InvalidDevice);
    }
    Ok(())
}

/// Ascending, unique, and never CLAP's invalid id.
pub(crate) fn valid_pins(pinned: &[u32]) -> bool {
    pinned.windows(2).all(|pair| pair[0] < pair[1]) && !pinned.contains(&u32::MAX)
}

pub(crate) fn validate_mixer(
    device: &Device,
    seen: &mut std::collections::HashSet<Id>,
) -> Result<(), ModelError> {
    if !seen.insert(device.id) {
        return Err(ModelError::DuplicateId);
    }
    if device.kind != DeviceKind::Mixer
        || device.state.is_some()
        || device.bypassed
        || device.offline
        || !device.pinned.is_empty()
        || device.parameters.len() != ParamKey::ORDER.len()
    {
        return Err(ModelError::InvalidParameter);
    }
    for (parameter, key) in device.parameters.iter().zip(ParamKey::ORDER) {
        if !seen.insert(parameter.id) {
            return Err(ModelError::DuplicateId);
        }
        if parameter.key != key {
            return Err(ModelError::InvalidParameter);
        }
        let (min, max, default) = key.ends();
        if parameter.min.to_bits() != min.to_bits()
            || parameter.max.to_bits() != max.to_bits()
            || parameter.default.to_bits() != default.to_bits()
            || !key.allows(parameter.value)
        {
            return Err(ModelError::InvalidParameter);
        }
    }
    Ok(())
}
