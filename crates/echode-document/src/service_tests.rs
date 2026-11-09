//! The worker's turns through a presenter with no C ABI and no drawing, which
//! is how a headless caller would use them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use echode_history::write_package;
use echode_model::{Id, Project};

use super::*;

const WAIT: Duration = Duration::from_secs(5);

struct View {
    tracks: usize,
    history: serde_json::Value,
    device: Vec<u8>,
}

#[derive(Default)]
struct Plain {
    panics: Arc<AtomicBool>,
}

impl Presenter for Plain {
    type View = View;

    fn present(&mut self, publication: crate::Publication<'_>) -> View {
        assert!(!self.panics.load(Ordering::SeqCst), "the presenter failed");
        View {
            tracks: publication.project.tracks().len(),
            history: serde_json::from_slice(&publication.history).unwrap(),
            device: publication.device,
        }
    }

    fn with_device_report(mut view: View, report: Vec<u8>) -> View {
        view.device = report;
        view
    }
}

fn package() -> PathBuf {
    let path = std::env::temp_dir().join(format!("echode-document-{}", Id::generate()));
    write_package(&path, &Project::new()).unwrap();
    path
}

fn submit(
    service: &DocumentService<View>,
    operation: Operation,
) -> Receiver<Result<View, DocumentError>> {
    let (sender, done) = channel();
    let ticket = Ticket::new(move |result| {
        let _ = sender.send(result);
    });
    service.submit(operation, ticket);
    done
}

fn finish(service: &DocumentService<View>, operation: Operation) -> Result<View, DocumentError> {
    submit(service, operation).recv_timeout(WAIT).unwrap()
}

fn add_track(name: &str) -> Operation {
    Operation::Perform(format!(r#"{{"type":"track.add","name":"{name}"}}"#).into_bytes())
}

#[test]
fn edits_undo_and_device_requests_take_their_turns_without_a_c_abi() {
    let path = package();
    let service = DocumentService::open(&path, Plain::default()).unwrap();
    let added = finish(&service, add_track("first")).unwrap();
    assert_eq!(added.tracks, 1);
    assert_eq!(added.history["changed"], true);
    // A device request completes from the lane with its report on the view.
    let close = format!(
        r#"{{"type":"device.gui.close","device":"{}"}}"#,
        Id::from_raw(0xD0)
    );
    let closed = finish(&service, Operation::Device(close.into_bytes())).unwrap();
    assert_eq!(closed.device, br#"{"closed":true}"#);
    assert_eq!(closed.tracks, 1);
    assert_eq!(finish(&service, Operation::Undo).unwrap().tracks, 0);
    assert_eq!(finish(&service, Operation::Redo).unwrap().tracks, 1);
    assert!(matches!(
        finish(&service, Operation::Invalid),
        Err(DocumentError::Rejected(crate::Rejected::UnknownOperation))
    ));
    let unchanged = finish(&service, Operation::Snapshot).unwrap();
    assert_eq!(unchanged.history["changed"], false);
    assert_eq!(unchanged.device, crate::EMPTY_DEVICE_REPORT);
    drop(service);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn a_panicking_turn_fails_its_request_and_the_next_ones_are_refused() {
    let path = package();
    let presenter = Plain::default();
    let panics = Arc::clone(&presenter.panics);
    let service = DocumentService::open(&path, presenter).unwrap();
    finish(&service, Operation::Snapshot).unwrap();
    panics.store(true, Ordering::SeqCst);
    assert!(matches!(
        finish(&service, add_track("panics")),
        Err(DocumentError::Panicked)
    ));
    panics.store(false, Ordering::SeqCst);
    assert!(matches!(
        finish(&service, Operation::Snapshot),
        Err(DocumentError::Stopped)
    ));
    service.shutdown();
    assert!(matches!(
        finish(&service, Operation::Snapshot),
        Err(DocumentError::Stopped)
    ));
    drop(service);
    let _ = std::fs::remove_dir_all(path);
}

/// A dead worker cannot tell anyone. Queued work still completes, a later
/// submission is refused and activity readers stop waiting.
#[test]
fn a_worker_that_dies_before_its_first_turn_ends_every_reader() {
    let path = package();
    let (start, started) = channel::<()>();
    let service = DocumentService::open_configured(
        &path,
        Plain::default(),
        Box::new(move |_| {
            let _ = started.recv();
            panic!("the worker dies with a command queued");
        }),
    )
    .unwrap();
    let queued = submit(&service, add_track("queued"));
    start.send(()).unwrap();
    assert!(matches!(
        queued.recv_timeout(WAIT).unwrap(),
        Err(DocumentError::Stopped)
    ));
    let page: serde_json::Value =
        serde_json::from_slice(&service.plugin_activity(0, WAIT)).unwrap();
    assert_eq!(page["closed"], true);
    assert!(matches!(
        finish(&service, Operation::Snapshot),
        Err(DocumentError::Stopped)
    ));
    assert!(!service.position().1);
    drop(service);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn contract_stop_submission_invalidates_queued_play_before_worker_receives_it() {
    let (sender, incoming) = channel();
    let service: DocumentService<View> = DocumentService {
        queue: Mutex::new(Some(sender)),
        thread: Mutex::new(None),
        lane_thread: None,
        live: Arc::new(LiveState::default()),
        cancellation: TransportCancellation::default(),
        activity: Arc::new(ActivityHub::new()),
        audition: Audition::start().unwrap(),
    };
    let audio = |json: &[u8]| {
        service.submit(Operation::Audio(json.to_vec()), Ticket::new(|_| {}));
    };
    audio(br#"{"type":"transport.play"}"#);
    audio(br#"{"type":"transport.stop"}"#);
    let play = incoming.try_recv().unwrap();
    let stop = incoming.try_recv().unwrap();
    // No worker has run, yet Stop must already cancel the earlier Play.
    assert!(!service.cancellation.is_current(play.transport_revision));
    assert!(service.cancellation.is_current(stop.transport_revision));
    audio(br#"{"type":"transport.play"}"#);
    let replay = incoming.try_recv().unwrap();
    assert!(service.cancellation.is_current(replay.transport_revision));
}
