//! Gain of 2.0, plus fault injection for the hosting spike.
//!
//! `ECHODE_TEST_FAULT=crash` aborts inside `process`. `ECHODE_TEST_FAULT=hang`
//! never returns from `process`. Both take down only this plugin's process.

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
}

pub struct TestGainProcessor<'a> {
    fault: Fault,
    host: HostSharedHandle<'a>,
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
            _ => Fault::None,
        };
        Ok(Self {
            fault,
            host: _host.shared(),
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        match self.fault {
            Fault::Crash => std::process::abort(),
            Fault::Hang => loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            },
            Fault::Callback => self.host.request_callback(),
            Fault::Restart => self.host.request_restart(),
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

clack_export_entry!(SinglePluginEntry<TestGain>);
