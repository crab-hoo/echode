//! Gain of 2.0, plus fault injection for the hosting spike.
//!
//! `ECHODE_TEST_FAULT=crash` aborts inside `process`. `ECHODE_TEST_FAULT=hang`
//! never returns from `process`. Both take down only this plugin's process.

use std::ffi::CStr;

use clack_plugin::entry::prelude::*;
use clack_plugin::prelude::*;

pub struct TestGain;

impl Plugin for TestGain {
    type AudioProcessor<'a> = TestGainProcessor<'a>;
    type Shared<'a> = ();
    type MainThread<'a> = TestMain;
}

impl DefaultPluginFactory for TestGain {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new("dev.echode.test-gain", "Echode Test Gain")
            .with_vendor("Echode")
            .with_version("0.0.0")
            .with_url("https://echode.invalid")
            .with_features([c"audio-effect"])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Self::Shared<'_>, PluginError> {
        Ok(())
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        _shared: &'a Self::Shared<'a>,
    ) -> Result<Self::MainThread<'a>, PluginError> {
        Ok(TestMain)
    }
}

pub struct TestMain;

impl PluginMainThread<'_, ()> for TestMain {
    fn on_main_thread(&self) {
        let mut stderr = std::io::stderr().lock();
        use std::io::Write;
        let _ = writeln!(stderr, "on-main-thread");
        let _ = stderr.flush();
    }
}

enum Fault {
    None,
    Crash,
    Hang,
    Callback,
    Restart,
    /// Writes `note-ons + note-offs * 16` and keeps the totals across blocks.
    Count,
    /// Writes the key of the latest note-off, or 0 when none has arrived.
    Key,
}

pub struct TestGainProcessor<'a> {
    fault: Fault,
    host: HostSharedHandle<'a>,
    seen_ons: u32,
    seen_offs: u32,
    /// Note id from the note-on. `u32::MAX` until one arrives.
    on_id: u32,
    /// Key of a note-off whose note id matches `on_id`.
    off_key: u8,
}

impl<'a> PluginAudioProcessor<'a, (), TestMain> for TestGainProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &TestMain,
        _shared: &'a (),
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let fault = match std::env::var("ECHODE_TEST_FAULT").as_deref() {
            Ok("crash") => Fault::Crash,
            Ok("hang") => Fault::Hang,
            Ok("callback") => Fault::Callback,
            Ok("restart") => Fault::Restart,
            Ok("count") => Fault::Count,
            Ok("key") => Fault::Key,
            _ => Fault::None,
        };
        Ok(Self {
            fault,
            host: _host.shared(),
            seen_ons: 0,
            seen_offs: 0,
            on_id: u32::MAX,
            off_key: 0,
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        match self.fault {
            Fault::Crash => std::process::abort(),
            Fault::Hang => loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            },
            Fault::Callback => self.host.request_callback(),
            Fault::Restart => self.host.request_restart(),
            Fault::Count => {
                for index in 0..events.input.len() as usize {
                    let Some(core) = events.input[index].as_core_event() else {
                        continue;
                    };
                    match core {
                        clack_plugin::events::spaces::CoreEventSpace::NoteOn(_) => {
                            self.seen_ons = self.seen_ons.saturating_add(1);
                        }
                        clack_plugin::events::spaces::CoreEventSpace::NoteOff(_) => {
                            self.seen_offs = self.seen_offs.saturating_add(1);
                        }
                        _ => {}
                    }
                }
                let value = self.seen_ons as f32 + self.seen_offs as f32 * 16.0;
                for mut port_pair in &mut audio {
                    let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                        continue;
                    };
                    for channel_pair in channel_pairs {
                        match channel_pair {
                            ChannelPair::InputOnly(_) => {}
                            ChannelPair::OutputOnly(buffer) => buffer.fill(value),
                            ChannelPair::InputOutput(_, output) => output.fill(value),
                            ChannelPair::InPlace(buffer) => buffer.fill(value),
                        }
                    }
                }
                return Ok(ProcessStatus::Continue);
            }
            Fault::Key => {
                for index in 0..events.input.len() as usize {
                    let Some(core) = events.input[index].as_core_event() else {
                        continue;
                    };
                    match core {
                        clack_plugin::events::spaces::CoreEventSpace::NoteOn(on) => {
                            if let clack_plugin::events::Match::Specific(id) = on.note_id() {
                                self.on_id = id;
                            }
                        }
                        clack_plugin::events::spaces::CoreEventSpace::NoteOff(off) => {
                            let Some(id) = off.note_id().into_specific() else {
                                continue;
                            };
                            if id != self.on_id {
                                continue;
                            }
                            self.off_key = off
                                .key()
                                .into_specific()
                                .and_then(|key| u8::try_from(key).ok())
                                .unwrap_or(0);
                        }
                        _ => {}
                    }
                }
                let value = f32::from(self.off_key);
                for mut port_pair in &mut audio {
                    let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                        continue;
                    };
                    for channel_pair in channel_pairs {
                        match channel_pair {
                            ChannelPair::InputOnly(_) => {}
                            ChannelPair::OutputOnly(buffer) => buffer.fill(value),
                            ChannelPair::InputOutput(_, output) => output.fill(value),
                            ChannelPair::InPlace(buffer) => buffer.fill(value),
                        }
                    }
                }
                return Ok(ProcessStatus::Continue);
            }
            Fault::None => {}
        }

        for mut port_pair in &mut audio {
            let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                continue;
            };
            for channel_pair in channel_pairs {
                match channel_pair {
                    ChannelPair::InputOnly(_) => {}
                    ChannelPair::OutputOnly(buffer) => buffer.fill(0.0),
                    ChannelPair::InputOutput(input, output) => {
                        for (input, output) in input.iter().zip(output) {
                            *output = input * 2.0;
                        }
                    }
                    ChannelPair::InPlace(buffer) => {
                        for sample in buffer {
                            *sample *= 2.0;
                        }
                    }
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }
}

/// The gain effect, plus an instrument that runs the same processor.
///
/// The instrument id exists so a project can hold both roles. The processor
/// still gains audio by 2; an instrument receives silence and notes, and
/// placed audio is added after it.
struct TestEntry {
    factory: PluginFactoryWrapper<TestFactory>,
}

impl Entry for TestEntry {
    fn new(_plugin_path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        Ok(Self {
            factory: PluginFactoryWrapper::new(TestFactory::new()),
        })
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        builder.register_factory(&self.factory);
    }
}

struct TestFactory {
    gain: PluginDescriptor,
    instrument: PluginDescriptor,
}

impl TestFactory {
    fn new() -> Self {
        Self {
            gain: TestGain::get_descriptor(),
            instrument: PluginDescriptor::new(
                "dev.echode.test-instrument",
                "Echode Test Instrument",
            )
            .with_vendor("Echode")
            .with_version("0.0.0")
            .with_url("https://echode.invalid")
            .with_features([c"instrument"]),
        }
    }
}

impl PluginFactoryImpl for TestFactory {
    fn plugin_count(&self) -> u32 {
        2
    }

    fn plugin_descriptor(&self, index: u32) -> Option<&PluginDescriptor> {
        match index {
            0 => Some(&self.gain),
            1 => Some(&self.instrument),
            _ => None,
        }
    }

    fn create_plugin<'a>(
        &'a self,
        host_info: HostInfo<'a>,
        plugin_id: &CStr,
    ) -> Option<PluginInstance<'a>> {
        let descriptor = if self.gain.id() == Some(plugin_id) {
            &self.gain
        } else if self.instrument.id() == Some(plugin_id) {
            &self.instrument
        } else {
            return None;
        };
        Some(PluginInstance::new::<TestGain>(
            host_info,
            descriptor,
            TestGain::new_shared,
            TestGain::new_main_thread,
        ))
    }
}

clack_export_entry!(TestEntry);
