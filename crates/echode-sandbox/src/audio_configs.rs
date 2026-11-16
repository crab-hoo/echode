//! Control-only summaries of CLAP audio-port presets and port requests. Never stored in a project.
use crate::PortLayout;
use crate::params::len32;
use crate::payload_decode::{Cursor, ParamField, ParamPayload};
use std::io;

/// Preset configurations returned by `clap.audio-ports-config` and optional
/// `clap.audio-ports-config-info`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPortConfigs {
    /// Active preset id from `clap.audio-ports-config-info::current_config`, if any.
    pub current_id: Option<u32>,
    pub configs: Vec<AudioPortConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPortConfig {
    /// The plugin's id, not its enumeration index.
    pub id: u32,
    pub name: String,
    pub input_ports: u32,
    pub output_ports: u32,
    /// Zero means the preset has no main input/output respectively.
    pub main_input_channels: u32,
    pub main_output_channels: u32,
    /// Per-port channel counts from `clap.audio-ports-config-info`, if supported.
    pub ports: Option<PortLayout>,
}

/// One requested port shape for `clap.configurable-audio-ports`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPortRequest {
    pub input: bool,
    pub index: u32,
    pub channels: u32,
    pub details: Option<AudioPortDetails>,
}
/// Owned control data. Only the helper turns these values into CLAP pointers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioPortDetails {
    Surround(Vec<u8>),
    Ambisonic { ordering: u32, normalization: u32 },
}

fn push_u32s(body: &mut Vec<u8>, values: &[u32]) {
    body.extend_from_slice(&len32(values.len()).to_le_bytes());
    for value in values {
        body.extend_from_slice(&value.to_le_bytes());
    }
}

pub(crate) fn encode(configs: &AudioPortConfigs) -> Vec<u8> {
    let mut body = Vec::new();
    match configs.current_id {
        Some(id) => {
            body.push(1);
            body.extend_from_slice(&id.to_le_bytes());
        }
        None => body.push(0),
    }
    body.extend_from_slice(&len32(configs.configs.len()).to_le_bytes());
    for config in &configs.configs {
        for value in [
            config.id,
            config.input_ports,
            config.output_ports,
            config.main_input_channels,
            config.main_output_channels,
        ] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        match &config.ports {
            Some(ports) => {
                body.push(1);
                push_u32s(&mut body, &ports.inputs);
                push_u32s(&mut body, &ports.outputs);
            }
            None => body.push(0),
        }
        body.extend_from_slice(&len32(config.name.len()).to_le_bytes());
        body.extend_from_slice(config.name.as_bytes());
    }
    body
}

pub(crate) fn decode(body: &[u8]) -> io::Result<AudioPortConfigs> {
    use ParamField::*;
    let mut cursor = Cursor::new(ParamPayload::AudioConfigs, body);
    let current_id = if cursor.flag(CurrentPresent)? {
        Some(cursor.u32(Id)?)
    } else {
        None
    };
    // Five integers, one ports flag byte, and one text length per entry.
    // Reservation is bounded by the message's actual bytes, not an untrusted count.
    let configs = cursor.list(Count, 25, |cursor, _| {
        let id = cursor.u32(Id)?;
        let input_ports = cursor.u32(InputPorts)?;
        let output_ports = cursor.u32(OutputPorts)?;
        let main_input_channels = cursor.u32(MainInputChannels)?;
        let main_output_channels = cursor.u32(MainOutputChannels)?;
        let ports = if cursor.flag(PortsPresent)? {
            let inputs = cursor.u32_vec(InputPortCount, InputPortChannels)?;
            let outputs = cursor.u32_vec(OutputPortCount, OutputPortChannels)?;
            Some(PortLayout::new(inputs, outputs))
        } else {
            None
        };
        let name = cursor.text(NameLength, Name)?;
        Ok(AudioPortConfig {
            id,
            name,
            input_ports,
            output_ports,
            main_input_channels,
            main_output_channels,
            ports,
        })
    })?;
    cursor.finish()?;
    Ok(AudioPortConfigs {
        current_id,
        configs,
    })
}

pub(crate) fn encode_requests(requests: &[AudioPortRequest]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + requests.len() * 10);
    body.extend_from_slice(&len32(requests.len()).to_le_bytes());
    for request in requests {
        body.push(u8::from(request.input));
        body.extend_from_slice(&request.index.to_le_bytes());
        body.extend_from_slice(&request.channels.to_le_bytes());
        match &request.details {
            None => body.push(0),
            Some(AudioPortDetails::Surround(map)) => {
                body.push(1);
                body.extend_from_slice(&len32(map.len()).to_le_bytes());
                body.extend_from_slice(map);
            }
            Some(AudioPortDetails::Ambisonic {
                ordering,
                normalization,
            }) => {
                body.push(2);
                body.extend_from_slice(&ordering.to_le_bytes());
                body.extend_from_slice(&normalization.to_le_bytes());
            }
        }
    }
    body
}

pub(crate) fn decode_requests(body: &[u8]) -> io::Result<Vec<AudioPortRequest>> {
    use ParamField::*;
    let mut cursor = Cursor::new(ParamPayload::AudioPortRequests, body);
    let requests = cursor.list(Count, 10, |cursor, _| {
        Ok(AudioPortRequest {
            input: cursor.flag(PortSide)?,
            index: cursor.u32(PortIndex)?,
            channels: cursor.u32(PortChannels)?,
            details: match cursor.u8(PortDetails)? {
                0 => None,
                1 => Some(AudioPortDetails::Surround(
                    cursor.bytes(ChannelMapCount, ChannelMap)?,
                )),
                2 => Some(AudioPortDetails::Ambisonic {
                    ordering: cursor.u32(AmbisonicOrdering)?,
                    normalization: cursor.u32(AmbisonicNormalization)?,
                }),
                value => return Err(crate::PayloadDecodeError::UnknownPortDetails(value).into()),
            },
        })
    })?;
    cursor.finish()?;
    Ok(requests)
}

pub(crate) fn encode_applicable(applicable: bool) -> Vec<u8> {
    vec![u8::from(applicable)]
}

pub(crate) fn decode_applicable(body: &[u8]) -> io::Result<bool> {
    let mut cursor = Cursor::new(ParamPayload::AudioPortsApplicable, body);
    let applicable = cursor.flag(ParamField::Applicable)?;
    cursor.finish()?;
    Ok(applicable)
}

#[cfg(test)]
#[path = "audio_configs_tests.rs"]
mod tests;
