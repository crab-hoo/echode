//! Exposes the process snapshot separately from timestamped transport changes.
use clack_plugin::events::Event;
use clack_plugin::events::event_types::TransportEvent;
use clack_plugin::events::io::InputEvents;
use clack_plugin::prelude::*;

pub(crate) fn render(
    process: Process<'_>,
    audio: &mut Audio<'_>,
    input: &InputEvents<'_>,
) -> Result<(), PluginError> {
    let initial = input
        .iter()
        .filter_map(|event| event.as_event::<TransportEvent>())
        .find(|event| event.time() == 0);
    let count = input
        .iter()
        .filter_map(|event| event.as_event::<TransportEvent>())
        .count();
    let snapshot = process.transport.map_or(-1.0, |event| event.tempo as f32);
    // Full equality includes flags, both timelines, loop bounds, bars and meter.
    let matched = if process.transport == initial {
        count as f32
    } else {
        -1.0
    };
    let mut channel = 0;
    for mut port in audio {
        let Some(channels) = port.channels()?.into_f32() else {
            continue;
        };
        for pair in channels {
            let output = match pair {
                ChannelPair::InputOnly(_) => continue,
                ChannelPair::OutputOnly(output)
                | ChannelPair::InputOutput(_, output)
                | ChannelPair::InPlace(output) => output,
            };
            output.fill(if channel == 0 { snapshot } else { matched });
            channel += 1;
        }
    }
    Ok(())
}
