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

/// Each active stage adds its timeline, so an omitted downstream relay is audible.
pub(crate) fn render_timeline(
    process: Process<'_>,
    audio: &mut Audio<'_>,
    input: &InputEvents<'_>,
    rate: f64,
) -> Result<(), PluginError> {
    use clack_plugin::events::event_types::TransportFlags;
    let initial = input
        .iter()
        .filter_map(|event| event.as_event::<TransportEvent>())
        .find(|event| event.time() == 0);
    let valid = initial.is_some() && process.transport == initial;
    let mut channel = 0;
    for mut port in audio {
        let Some(channels) = port.channels()?.into_f32() else {
            continue;
        };
        for pair in channels {
            let mut updates = input
                .iter()
                .filter_map(|event| event.as_event::<TransportEvent>())
                .peekable();
            let mut current = process.transport;
            let mut value = |frame: usize| {
                while updates
                    .peek()
                    .is_some_and(|event| event.time() as usize <= frame)
                {
                    current = updates.next();
                }
                let Some(event) = current.filter(|_| valid) else {
                    return -32.0;
                };
                let playing = event.flags.contains(TransportFlags::IS_PLAYING);
                if channel == 0 {
                    let beats = f64::from(event.song_pos_beats);
                    ((beats
                        + if playing {
                            (frame as f64 - f64::from(event.time())) * event.tempo / (60.0 * rate)
                        } else {
                            0.0
                        })
                        / 16.0) as f32
                } else {
                    (event.tempo / 1024.0 + if playing { 1.0 } else { 0.0 }) as f32
                }
            };
            match pair {
                ChannelPair::InputOnly(_) => {}
                ChannelPair::InputOutput(input, output) => {
                    for (frame, (input, output)) in input.iter().zip(output).enumerate() {
                        *output = *input + value(frame);
                    }
                }
                ChannelPair::OutputOnly(output) => {
                    for (frame, output) in output.iter_mut().enumerate() {
                        *output = value(frame);
                    }
                }
                ChannelPair::InPlace(output) => {
                    for (frame, output) in output.iter_mut().enumerate() {
                        *output += value(frame);
                    }
                }
            }
            channel += 1;
        }
    }
    Ok(())
}
