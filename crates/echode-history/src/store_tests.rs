use super::*;

#[test]
fn stopped_worker_channels_keep_their_typed_cause() {
    let (reply, incoming) = mpsc::channel();
    drop(reply);
    let error = PendingSave { reply: incoming }.wait().unwrap_err();
    assert!(matches!(error, PackageError::Spike(SpikeError::Stopped)));
    assert_eq!(error.to_string(), "history thread stopped");

    let (request, incoming) = mpsc::channel();
    drop(incoming);
    let history = History {
        request,
        thread: None,
    };
    let error = history
        .save(Arc::new(Project { tracks: Vec::new() }))
        .err()
        .unwrap();
    assert!(matches!(error, PackageError::Spike(SpikeError::Stopped)));
    let error = history
        .load(ObjectId::null(gix::hash::Kind::Sha1))
        .unwrap_err();
    assert!(matches!(error, PackageError::Spike(SpikeError::Stopped)));
}

#[test]
fn receive_deadline_is_not_a_stopped_worker() {
    use std::sync::mpsc::RecvTimeoutError;
    let timeout = SpikeError::from(RecvTimeoutError::Timeout);
    let stopped = SpikeError::from(RecvTimeoutError::Disconnected);
    assert!(matches!(timeout, SpikeError::TimedOut));
    assert!(matches!(stopped, SpikeError::Stopped));
    assert_eq!(timeout.to_string(), "history thread stopped");
    assert_eq!(stopped.to_string(), "history thread stopped");
}
