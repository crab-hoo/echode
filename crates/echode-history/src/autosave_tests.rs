use super::*;
use std::error::Error;

#[test]
fn queued_saves_collapse_to_the_latest() {
    let (sender, incoming) = mpsc::channel();
    let mut project = Project::new();
    let mut first = None;
    for seq in 1..=3 {
        let (next, _) = project.add_track(format!("t{seq}"));
        project = next;
        let (reply, _) = mpsc::channel();
        let request = Request::Save {
            project: Arc::new(project.clone()),
            journal_seq: seq,
            reply,
        };
        if first.is_none() {
            first = Some(request);
        } else {
            sender.send(request).unwrap();
        }
    }
    sender.send(Request::Shutdown).unwrap();
    let Request::Save {
        project,
        journal_seq,
        reply,
    } = first.unwrap()
    else {
        unreachable!("the first request is a save");
    };
    let coalesced = coalesce_save(project, journal_seq, reply, &incoming);
    assert_eq!(coalesced.journal_seq, 3);
    assert_eq!(coalesced.project.tracks().len(), 3);
    assert_eq!(coalesced.replies.len(), 3);
    assert!(matches!(coalesced.next, Some(Request::Shutdown)));
}

#[test]
fn coalesced_failures_share_the_original_and_keep_durability() {
    for error in [
        PackageError::durability_unknown(),
        PackageError::from(std::io::Error::from_raw_os_error(libc::EACCES)),
        PackageError::git(
            GitOperation::WriteObject,
            gix::Error::from_error(std::io::Error::from_raw_os_error(libc::ENOSPC)),
        ),
    ] {
        let original = error.into_shared();
        let (sender, incoming) = mpsc::channel();
        let (first_reply, first) = mpsc::channel();
        let (second_reply, second) = mpsc::channel();
        let (abandoned_reply, abandoned) = mpsc::channel();
        drop(abandoned);
        for reply in [abandoned_reply, second_reply] {
            sender
                .send(Request::Save {
                    project: Arc::new(Project::new()),
                    journal_seq: 2,
                    reply,
                })
                .unwrap();
        }
        let coalesced = coalesce_save(Arc::new(Project::new()), 1, first_reply, &incoming);
        answer(
            coalesced.replies,
            Err(PackageError::Shared(Arc::clone(&original))),
        );
        for reply in [first, second] {
            let delivered = reply.recv().unwrap().unwrap_err();
            assert_eq!(delivered.to_string(), original.to_string());
            assert_eq!(
                delivered.is_durability_unknown(),
                original.is_durability_unknown()
            );
            let PackageError::Shared(shared) = &delivered else {
                panic!("coalesced reply lost its original cause: {delivered:?}");
            };
            assert!(Arc::ptr_eq(shared, &original));
            assert!(delivered.source().unwrap().is::<PackageError>());
        }
    }
}

#[test]
fn pending_replies_distinguish_empty_completed_and_disconnected() {
    let (reply, incoming) = mpsc::channel();
    let pending = PendingAutosave { reply: incoming };
    assert!(pending.try_wait().is_none());
    reply.send(Err(PackageError::durability_unknown())).unwrap();
    assert!(
        pending
            .try_wait()
            .unwrap()
            .unwrap_err()
            .is_durability_unknown()
    );
    drop(reply);
    assert!(matches!(
        pending.try_wait(),
        Some(Err(PackageError::Autosave(AutosaveError::Stopped)))
    ));
    assert!(matches!(
        pending.wait(),
        Err(PackageError::Autosave(AutosaveError::Stopped))
    ));
}
