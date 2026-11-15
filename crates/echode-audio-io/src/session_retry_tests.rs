use super::*;
use std::time::Duration;

#[derive(Default)]
struct InventorySource {
    mode: AtomicU64,
    reads: AtomicU64,
}

impl Outputs for InventorySource {
    fn refresh(
        &self,
        _: &Arc<AtomicU64>,
    ) -> Result<(Inventory, Option<hardware::Notifications>), AudioError> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        match self.mode.load(Ordering::Acquire) {
            1 => Err(crate::HardwareFault::SampleRate.into()),
            2 => Err(crate::HardwareFault::ListKeptChanging.into()),
            mode => Ok((
                Inventory {
                    default_device: if mode == 0 { 42 } else { 84 },
                    devices: Vec::new(),
                },
                None,
            )),
        }
    }

    fn claim(&self, _: OutputDevice) -> Result<crate::stream::Claim, AudioError> {
        Err(AudioError::NoOutputDevice)
    }

    fn register(&self, _: &mut OutputStream) -> Result<(), AudioError> {
        Ok(())
    }
}

#[test]
fn inventory_retries_keep_the_last_state_and_log_only_changed_failures() {
    let capture = echode_log::testing::capture();
    let source = Arc::new(InventorySource::default());
    let mut session = Session::new(Arc::new(Project::new()));
    session.outputs = source.clone();
    session
        .perform(AudioCommand::Output {
            uid: Some("kept selection".into()),
        })
        .unwrap();
    session
        .perform(AudioCommand::SetStart { tick: 77 })
        .unwrap();
    source.mode.store(1, Ordering::Release);
    session.hardware_epoch.fetch_add(1, Ordering::AcqRel);
    let now = Instant::now();
    session.service_at(now);
    assert_eq!(source.reads.load(Ordering::Acquire), 2);
    let error = session.state().error;
    assert!(error.is_some());
    for _ in 0..100 {
        // More notifications do not turn a failed read into a busy loop.
        session.hardware_epoch.fetch_add(1, Ordering::AcqRel);
        session.service_at(now + Duration::from_millis(99));
    }
    assert_eq!(source.reads.load(Ordering::Acquire), 2);
    assert_eq!(session.inventory.default_device, 42);
    assert_eq!(
        session.state().selected_uid.as_deref(),
        Some("kept selection")
    );
    assert_eq!((session.state().tick, session.state().start), (77, 77));
    assert_eq!(session.state().error, error);
    session.service_at(now + Duration::from_millis(100));
    assert_eq!(source.reads.load(Ordering::Acquire), 3);
    source.mode.store(2, Ordering::Release);
    session.service_at(now + Duration::from_millis(300));
    assert_eq!(source.reads.load(Ordering::Acquire), 4);
    assert_ne!(session.state().error, error);
    let warnings = || {
        capture
            .on_this_thread()
            .into_iter()
            .filter(|record| record["target"] == "echode_audio_io::session")
            .count()
    };
    assert_eq!(warnings(), 2);
    source.mode.store(3, Ordering::Release);
    session.service_at(now + Duration::from_millis(699));
    assert_eq!(source.reads.load(Ordering::Acquire), 4);
    session.service_at(now + Duration::from_millis(700));
    assert_eq!(session.inventory.default_device, 84);
    assert_eq!(
        session.observed_epoch,
        session.hardware_epoch.load(Ordering::Acquire)
    );
    assert!(session.state().error.is_none());
    assert_eq!(warnings(), 2);
    source.mode.store(1, Ordering::Release);
    session.hardware_epoch.fetch_add(1, Ordering::AcqRel);
    session.service_at(now + Duration::from_millis(700));
    assert_eq!(warnings(), 3, "success starts a new failure episode");
    session.service_at(now + Duration::from_millis(800));
    assert_eq!(source.reads.load(Ordering::Acquire), 7);
}

#[test]
fn even_the_first_failed_read_recovers_without_another_hardware_notification() {
    let source = Arc::new(InventorySource::default());
    source.mode.store(1, Ordering::Release);
    let mut session = Session::new(Arc::new(Project::new()));
    session.outputs = source.clone();
    let now = Instant::now();
    assert!(session.refresh_with_clock(|| now).is_err());
    source.mode.store(3, Ordering::Release);
    session.service_at(now + Duration::from_millis(99));
    assert_eq!(source.reads.load(Ordering::Acquire), 1);
    session.service_at(now + Duration::from_millis(100));
    assert_eq!(source.reads.load(Ordering::Acquire), 2);
    assert!(session.refreshed);
    assert!(session.state().error.is_none());
}
