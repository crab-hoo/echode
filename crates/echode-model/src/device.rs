//! Built-in mixer parameters. Plugin devices are not here yet.

use crate::id::Id;
use crate::model::ModelError;

/// Linear amplitude of about +12 dB, the fader's upper end.
pub const GAIN_MAX: f32 = 4.0;

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

    fn all(self) -> [Id; 5] {
        [self.device, self.gain, self.pan, self.mute, self.solo]
    }
}

/// A built-in device. Only the mixer exists in this slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Mixer,
}

impl DeviceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mixer => "mixer",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "mixer" => Some(Self::Mixer),
            _ => None,
        }
    }
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
            Self::Pan => (-1.0..=1.0).contains(&value),
            Self::Mute | Self::Solo => value == 0.0 || value == 1.0,
        }
    }

    pub(crate) fn ends(self) -> (f32, f32, f32) {
        match self {
            Self::Gain => (0.0, GAIN_MAX, 1.0),
            Self::Pan => (-1.0, 1.0, 0.0),
            Self::Mute | Self::Solo => (0.0, 1.0, 0.0),
        }
    }
}

/// One addressable value. Equality uses float bits.
#[derive(Clone, Debug)]
pub struct Parameter {
    pub id: Id,
    pub key: ParamKey,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub value: f32,
}

impl PartialEq for Parameter {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.key == other.key
            && self.min.to_bits() == other.min.to_bits()
            && self.max.to_bits() == other.max.to_bits()
            && self.default.to_bits() == other.default.to_bits()
            && self.value.to_bits() == other.value.to_bits()
    }
}

impl Eq for Parameter {}

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

/// A device on a track. Parameters stay in [`ParamKey::ORDER`] for a mixer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: Id,
    pub kind: DeviceKind,
    pub parameters: Vec<Parameter>,
}

impl Device {
    pub fn unity(ids: MixerIds) -> Self {
        Self {
            id: ids.device,
            kind: DeviceKind::Mixer,
            parameters: vec![
                Parameter::at_default(ids.gain, ParamKey::Gain),
                Parameter::at_default(ids.pan, ParamKey::Pan),
                Parameter::at_default(ids.mute, ParamKey::Mute),
                Parameter::at_default(ids.solo, ParamKey::Solo),
            ],
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

pub(crate) fn validate_mixer(
    device: &Device,
    seen: &mut std::collections::HashSet<Id>,
) -> Result<(), ModelError> {
    if !seen.insert(device.id) {
        return Err(ModelError::DuplicateId);
    }
    if device.kind != DeviceKind::Mixer || device.parameters.len() != ParamKey::ORDER.len() {
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

pub(crate) fn mixer_ids_free(ids: MixerIds, occupied: impl Fn(Id) -> bool) -> bool {
    let all = ids.all();
    all.iter().all(|id| !occupied(*id))
        && all.iter().collect::<std::collections::HashSet<_>>().len() == all.len()
}
