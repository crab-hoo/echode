use super::*;

#[test]
fn audition_service_and_close_report_device_overloads_without_hardware() {
    let capture = echode_log::testing::capture();
    let (control, _) = audition_pair(48_000).unwrap();
    let (_, heard) = heard_notes();
    let mut outputs = HalOutputs {
        open: Some((
            None,
            AuditionStream {
                device: OutputDevice {
                    id: 416,
                    uid: "audition-overload".into(),
                    name: "audition-overload".into(),
                    sample_rate: 48_000,
                    buffer_frames: 128,
                    safety_frames: 0,
                    channels: 2,
                    formats: Vec::new(),
                },
                overloads: Some(crate::overload::Listener::unregistered(416, "audition")),
                control,
                heard,
                callback: None,
                proc: None,
                observed: 0,
            },
        )),
        ..Default::default()
    };
    outputs
        .open
        .as_ref()
        .unwrap()
        .1
        .overloads
        .as_ref()
        .unwrap()
        .notify();
    assert!(outputs.control().is_some());
    let records = capture.on_this_thread();
    assert_eq!(records.len(), 1);
    assert!(
        records[0]["message"]
            .as_str()
            .unwrap()
            .contains("source=audition device=416")
    );
    assert_eq!(outputs.epoch.load(Ordering::Acquire), 0);
    outputs
        .open
        .as_ref()
        .unwrap()
        .1
        .overloads
        .as_ref()
        .unwrap()
        .notify();
    outputs.close();
    assert_eq!(capture.on_this_thread().len(), 2);
    outputs.close();
    assert_eq!(capture.on_this_thread().len(), 2);
}
