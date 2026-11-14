//! Capture failures through the product device lane and worker completion.
//! No plugin or output device is needed to inject these typed replies.

use std::sync::mpsc::{TryRecvError, channel};
use std::time::Instant;

use echode_audio_io::{Refusal, SessionError};
use echode_model::Project;

use super::*;
use crate::Publication;

struct Plain;

impl Presenter for Plain {
    type View = Vec<u8>;

    fn present(&mut self, publication: Publication<'_>) -> Vec<u8> {
        publication.device
    }

    fn with_device_report(_view: Vec<u8>, report: Vec<u8>) -> Vec<u8> {
        report
    }
}

fn worker(host: DocumentHost) -> Worker<Plain> {
    Worker {
        audio: Session::new(host.snapshot()),
        lane: DeviceLane::start(Plain::with_device_report).unwrap(),
        readers: Readers {
            live: Arc::new(LiveState::default()),
            activity: Arc::new(ActivityHub::new()),
        },
        host,
        presenter: Plain,
        saves: PluginSaves::default(),
        waiting: VecDeque::new(),
        stage: crate::import::product_stage(),
        importing: None,
        held: VecDeque::new(),
        accepting: true,
        publishes_live: true,
    }
}

/// DIA-02: a helper's failure must take the same completion path as captured
/// bytes. Propagating it before that path would leave the edit open forever.
#[test]
fn failed_captures_release_background_saves_and_preserve_their_typed_error() {
    for failure in [
        DocumentError::Helper(SessionError::Timeout),
        DocumentError::Helper(SessionError::Refused(Refusal::Busy)),
        DocumentError::DeviceRequestPanicked,
    ] {
        let dir = std::env::temp_dir().join(format!("echode-document-capture-{}", Id::generate()));
        let (project, track) = Project::new().add_track("fx");
        let device = Id::from_raw(0xD4);
        let other = Id::from_raw(0xD5);
        let project = project
            .add_clap_device(track, device, "org.example.fx")
            .unwrap()
            .add_clap_device(track, other, "org.example.fx")
            .unwrap();
        echode_history::write_package(&dir, &project).unwrap();
        let mut worker = worker(DocumentHost::open(&dir).unwrap());
        worker.saves.editing.extend([device.raw(), other.raw()]);
        worker.saves.add(vec![
            (device.raw(), b"edited".to_vec()),
            (other.raw(), b"other".to_vec()),
        ]);
        worker.record_plugin_saves();
        assert!(
            worker.host.snapshot().tracks()[0].devices[0]
                .state
                .is_none()
        );
        assert_eq!(worker.saves.held().len(), 2);
        let (finished, done) = channel();
        let ticket = Ticket::new(move |result| finished.send(result).unwrap());
        worker
            .lane
            .capture(ticket, device, Box::new(|| Err(failure)));
        let deadline = Instant::now() + Duration::from_secs(5);
        let result = loop {
            worker.record_returned();
            match done.try_recv() {
                Ok(result) => break result,
                Err(TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline, "the capture did not complete");
                    std::thread::yield_now();
                }
                Err(TryRecvError::Disconnected) => panic!("the capture lost its completion"),
            }
        };
        assert!(matches!(
            result,
            Err(DocumentError::Helper(SessionError::Timeout))
                | Err(DocumentError::Helper(SessionError::Refused(Refusal::Busy)))
                | Err(DocumentError::DeviceRequestPanicked)
        ));
        assert!(
            !worker.saves.editing.contains(&device.raw()),
            "the failed capture still holds the device's background saves"
        );
        assert!(worker.saves.editing.contains(&other.raw()));
        assert_eq!(worker.host.history_usage().unwrap().undo_steps, 0);
        // No new commit or dirty notification: the save already waiting can land.
        worker.record_plugin_saves();
        assert_eq!(
            worker.host.snapshot().tracks()[0].devices[0].state,
            Some(echode_history::object_hash(b"edited"))
        );
        assert!(
            worker.host.snapshot().tracks()[0].devices[1]
                .state
                .is_none()
        );
        assert_eq!(worker.host.history_usage().unwrap().undo_steps, 0);
        drop(worker);
        let reopened = DocumentHost::open(&dir).unwrap();
        assert_eq!(
            reopened.snapshot().tracks()[0].devices[0].state,
            Some(echode_history::object_hash(b"edited"))
        );
        drop(reopened);
        let _ = std::fs::remove_dir_all(dir);
    }
}
