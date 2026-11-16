//! A key-input probe, not a compressor: every supplied key sample is observable.
use clack_plugin::prelude::*;

pub(crate) fn render(audio: &mut Audio<'_>) -> Result<(), PluginError> {
    // These two slots describe this stereo fixture only, not a product limit.
    let mut outputs: [Option<&mut [f32]>; 2] = [None, None];
    for (port_index, mut port) in audio.into_iter().enumerate() {
        let Some(channels) = port.channels()?.into_f32() else {
            continue;
        };
        for (channel, pair) in channels.into_iter().enumerate() {
            match (port_index, pair) {
                (0, ChannelPair::InputOutput(input, output)) => {
                    output.copy_from_slice(input);
                    outputs[channel] = Some(output);
                }
                (0, ChannelPair::InPlace(output)) => {
                    outputs[channel] = Some(output);
                }
                (1, ChannelPair::InputOnly(input)) => {
                    if let Some(output) = &mut outputs[channel] {
                        for (output, input) in output.iter_mut().zip(input) {
                            *output += input * 4.0;
                        }
                    }
                }
                (2, ChannelPair::InputOnly(input)) => {
                    if input.iter().any(|&sample| sample != 0.0) {
                        return Err(PluginError::Message(
                            "unselected auxiliary input is not silent",
                        ));
                    }
                }
                _ => return Err(PluginError::Message("unexpected key probe ports")),
            }
        }
    }
    Ok(())
}
