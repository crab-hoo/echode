//! Control-only summaries of CLAP audio-port presets. Never stored in a project.
use crate::params::len32;
use crate::payload_decode::{Cursor, ParamField, ParamPayload};
use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPortConfig {
    /// The plugin's id, not its enumeration index.
    pub id: u32,
    pub name: String,
    pub input_ports: u32,
    pub output_ports: u32,
    /// Zero means the preset has no main input/output respectively. Exact port
    /// boundaries are always read from audio-ports after selecting the preset.
    pub main_input_channels: u32,
    pub main_output_channels: u32,
}

pub(crate) fn encode(configs: &[AudioPortConfig]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&len32(configs.len()).to_le_bytes());
    for config in configs {
        for value in [
            config.id,
            config.input_ports,
            config.output_ports,
            config.main_input_channels,
            config.main_output_channels,
        ] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        body.extend_from_slice(&len32(config.name.len()).to_le_bytes());
        body.extend_from_slice(config.name.as_bytes());
    }
    body
}

pub(crate) fn decode(body: &[u8]) -> io::Result<Vec<AudioPortConfig>> {
    use ParamField::*;
    let mut cursor = Cursor::new(ParamPayload::AudioConfigs, body);
    // Five integers and one text length per entry. Reservation is bounded by
    // the message's actual bytes, not by an untrusted count.
    let configs = cursor.list(Count, 24, |cursor, _| {
        Ok(AudioPortConfig {
            id: cursor.u32(Id)?,
            input_ports: cursor.u32(InputPorts)?,
            output_ports: cursor.u32(OutputPorts)?,
            main_input_channels: cursor.u32(MainInputChannels)?,
            main_output_channels: cursor.u32(MainOutputChannels)?,
            name: cursor.text(NameLength, Name)?,
        })
    })?;
    cursor.finish()?;
    Ok(configs)
}

#[cfg(test)]
#[path = "audio_configs_tests.rs"]
mod tests;
