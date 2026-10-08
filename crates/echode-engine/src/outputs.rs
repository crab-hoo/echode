//! Which output device should be open.
//!
//! The audio thread never decides this. A control thread applies hardware
//! events here, then restarts the device if the active id changed.

/// The user's output setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputChoice {
    /// Follow the system default, including headphones being plugged in.
    SystemDefault,
    /// Stay on this device while CoreAudio still lists it.
    Device(u32),
}

/// What the control thread should do after an event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RouteUpdate {
    Unchanged,
    /// Open this device. The previous one, if any, is no longer active.
    Open(u32),
    /// No listed device can play.
    Closed,
    /// The active device kept its identity, but its rate or buffer changed.
    Reconfigure(DeviceFormat),
}

/// Sample rate and buffer size reported by the active device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceFormat {
    pub sample_rate: f64,
    pub buffer_frames: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputRoute {
    choice: OutputChoice,
    active: Option<u32>,
    default_device: u32,
    available: Vec<u32>,
}

impl OutputRoute {
    pub fn new(
        choice: OutputChoice,
        available: Vec<u32>,
        default_device: u32,
    ) -> (Self, RouteUpdate) {
        let mut route = Self {
            choice,
            active: None,
            default_device,
            available: unique(available),
        };
        let update = route.publish();
        (route, update)
    }

    pub fn choice(&self) -> OutputChoice {
        self.choice
    }

    pub fn active(&self) -> Option<u32> {
        self.active
    }

    pub fn default_device(&self) -> u32 {
        self.default_device
    }

    pub fn set_choice(&mut self, choice: OutputChoice) -> RouteUpdate {
        self.choice = choice;
        self.publish()
    }

    /// Replace the device list. A chosen device that reappears becomes active again.
    pub fn note_available(&mut self, available: Vec<u32>) -> RouteUpdate {
        self.available = unique(available);
        self.publish()
    }

    pub fn note_default(&mut self, default_device: u32) -> RouteUpdate {
        self.default_device = default_device;
        self.publish()
    }

    pub fn note_format(&mut self, device: u32, format: DeviceFormat) -> RouteUpdate {
        if self.active == Some(device) {
            RouteUpdate::Reconfigure(format)
        } else {
            RouteUpdate::Unchanged
        }
    }

    fn publish(&mut self) -> RouteUpdate {
        let desired = self.desired();
        if desired == self.active {
            return RouteUpdate::Unchanged;
        }
        self.active = desired;
        match desired {
            Some(device) => RouteUpdate::Open(device),
            None => RouteUpdate::Closed,
        }
    }

    fn desired(&self) -> Option<u32> {
        match self.choice {
            OutputChoice::SystemDefault => self.listed(self.default_device),
            OutputChoice::Device(id) => {
                self.listed(id).or_else(|| self.listed(self.default_device))
            }
        }
    }

    fn listed(&self, device: u32) -> Option<u32> {
        self.available.contains(&device).then_some(device)
    }
}

fn unique(devices: Vec<u32>) -> Vec<u32> {
    let mut unique = Vec::new();
    for device in devices {
        if !unique.contains(&device) {
            unique.push(device);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(choice: OutputChoice) -> OutputRoute {
        let (route, update) = OutputRoute::new(choice, vec![1, 2], 1);
        let expected = match choice {
            OutputChoice::SystemDefault => 1,
            OutputChoice::Device(id) => id,
        };
        assert_eq!(update, RouteUpdate::Open(expected));
        route
    }

    #[test]
    fn the_system_default_follows_headphones() {
        let mut route = route(OutputChoice::SystemDefault);
        assert_eq!(route.note_default(2), RouteUpdate::Open(2));
        assert_eq!(route.active(), Some(2));
        assert_eq!(route.note_default(2), RouteUpdate::Unchanged);
    }

    #[test]
    fn a_chosen_device_stays_until_it_disappears_and_returns() {
        let mut route = route(OutputChoice::SystemDefault);
        assert_eq!(
            route.set_choice(OutputChoice::Device(2)),
            RouteUpdate::Open(2)
        );
        assert_eq!(route.note_default(1), RouteUpdate::Unchanged);
        assert_eq!(route.active(), Some(2));

        assert_eq!(route.note_available(vec![1]), RouteUpdate::Open(1));
        assert_eq!(route.choice(), OutputChoice::Device(2));
        assert_eq!(route.note_available(vec![1, 2]), RouteUpdate::Open(2));
    }

    #[test]
    fn losing_every_device_closes_until_one_returns() {
        let mut route = route(OutputChoice::SystemDefault);
        assert_eq!(route.note_available(Vec::new()), RouteUpdate::Closed);
        assert_eq!(route.active(), None);
        route.note_default(4);
        assert_eq!(route.note_available(vec![4]), RouteUpdate::Open(4));
    }

    #[test]
    fn only_the_active_device_reconfigures_playback() {
        let mut route = route(OutputChoice::Device(2));
        let format = DeviceFormat {
            sample_rate: 44_100.0,
            buffer_frames: 128,
        };
        assert_eq!(route.note_format(1, format), RouteUpdate::Unchanged);
        assert_eq!(
            route.note_format(2, format),
            RouteUpdate::Reconfigure(format)
        );
    }

    #[test]
    #[ignore = "needs a CoreAudio output device; `make spike-engine` runs it"]
    fn the_live_inventory_includes_the_default_output() {
        let inventory = crate::device::output_inventory().expect("output devices");
        assert!(
            inventory.outputs.contains(&inventory.default_device),
            "default {} is not in {:?}",
            inventory.default_device,
            inventory.outputs
        );
    }
}
