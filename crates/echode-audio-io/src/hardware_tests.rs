use super::*;
use std::cell::{Cell, RefCell};

struct Reader {
    broken: Cell<Option<u32>>,
    fault: AudioError,
    system_failure: u8,
    reads: RefCell<Vec<u32>>,
}

impl InventoryReader for Reader {
    fn default_device(&self) -> Result<u32, AudioError> {
        if self.system_failure == 1 {
            return Err(self.fault.clone());
        }
        Ok(2)
    }

    fn devices(&self) -> Result<Vec<u32>, AudioError> {
        if self.system_failure == 2 {
            return Err(self.fault.clone());
        }
        if self.system_failure == 3 {
            return Ok(Vec::new());
        }
        Ok(vec![1, 2, 3, 4])
    }

    fn output(&self, id: u32) -> Result<Option<OutputDevice>, AudioError> {
        self.reads.borrow_mut().push(id);
        if self.system_failure == 4 || self.broken.get() == Some(id) {
            return Err(self.fault.clone());
        }
        if id == 4 {
            return Ok(None); // An input-only or dead device is not an error.
        }
        Ok(Some(OutputDevice {
            id,
            uid: format!("uid-{id}"),
            name: format!("output-{id}"),
            sample_rate: 48_000,
            buffer_frames: 128,
            safety_frames: 0,
            channels: 2,
            formats: Vec::new(),
        }))
    }
}

#[test]
fn audit_iqa06_one_unreadable_device_keeps_other_outputs_and_recovers() {
    let capture = echode_log::testing::capture();
    for fault in [
        AudioError::CoreAudio {
            call: CoreAudioCall::Property,
            status: -50,
        },
        HardwareFault::SampleRate.into(),
        HardwareFault::ChannelOverflow.into(),
        HardwareFault::EmptyName.into(),
    ] {
        for broken in [1, 2, 3] {
            let reader = Reader {
                broken: Cell::new(Some(broken)),
                fault: fault.clone(),
                system_failure: 0,
                reads: RefCell::new(Vec::new()),
            };
            let result = inventory_with(&reader);
            assert!(
                result.is_ok(),
                "IQA-06: one device failure discarded healthy outputs: {result:?}"
            );
            let inventory = result.unwrap();
            assert_eq!(inventory.default_device, 2);
            assert_eq!(*reader.reads.borrow(), [1, 2, 3, 4]);
            assert_eq!(inventory.devices.len(), 2);
            assert!(inventory.devices.iter().all(|device| device.id != broken));
            let selected = crate::session::desired(&inventory, Some("uid-1")).unwrap();
            assert_eq!(selected.id, if broken == 1 { 2 } else { 1 });
            let last = capture.on_this_thread().pop().unwrap();
            assert_eq!(last["level"], "warn");
            assert!(
                last["message"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("device={broken}"))
            );
            assert!(
                last["message"]
                    .as_str()
                    .unwrap()
                    .contains(&fault.to_string())
            );
            reader.broken.set(None);
            assert_eq!(inventory_with(&reader).unwrap().devices.len(), 3);
        }
    }
}

#[test]
fn system_inventory_failures_still_fail_and_empty_or_all_unreadable_sets_are_valid() {
    for system_failure in [1, 2] {
        let reader = Reader {
            broken: Cell::new(None),
            fault: HardwareFault::ListKeptChanging.into(),
            system_failure,
            reads: RefCell::new(Vec::new()),
        };
        assert!(matches!(
            inventory_with(&reader),
            Err(AudioError::Hardware(HardwareFault::ListKeptChanging))
        ));
        assert!(reader.reads.borrow().is_empty());
    }
    for system_failure in [3, 4] {
        let reader = Reader {
            broken: Cell::new(None),
            fault: HardwareFault::SampleRate.into(),
            system_failure,
            reads: RefCell::new(Vec::new()),
        };
        let inventory = inventory_with(&reader).unwrap();
        assert!(inventory.devices.is_empty());
        assert!(crate::session::desired(&inventory, None).is_none());
    }
}
