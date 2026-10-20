use echode_actions::{Command, Session, decode, encode, replay};
use echode_history::{read_package, write_package};
use echode_model::{Id, MixerIds, ModelError, Project};

fn insert(session: &mut Session, after: Id, into_folder: bool, name: &str) -> Id {
    let command: Command = serde_json::from_value(serde_json::json!({
        "type": "track.insert", "after": after, "into_folder": into_folder, "name": name,
    }))
    .expect("the product accepts positioned track insertion");
    session.perform(command).unwrap();
    session.project().check_invariants().unwrap();
    session
        .project()
        .tracks()
        .iter()
        .find(|track| track.name == name)
        .unwrap()
        .id
}

#[test]
fn positioned_tracks_preserve_sibling_keys_and_survive_history_and_reopen() {
    let (base, folder) = Project::new().add_track("folder");
    let (base, child) = base.add_child(folder, "child").unwrap();
    let (base, nested) = base.add_child(child, "nested").unwrap();
    let (base, tail) = base.add_track("tail");
    let mut session = Session::open(base.clone());
    let inside = insert(&mut session, folder, true, "inside");
    assert_eq!(session.project().parent(inside).unwrap(), Some(folder));
    let outside = insert(&mut session, folder, false, "outside");
    assert_eq!(session.project().parent(outside).unwrap(), None);
    let sibling = insert(&mut session, child, false, "sibling");
    assert_eq!(session.project().parent(sibling).unwrap(), Some(folder));
    assert_eq!(
        session
            .project()
            .tracks()
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>(),
        [folder, child, nested, sibling, inside, outside, tail]
    );
    for old in base.tracks() {
        let kept = session
            .project()
            .tracks()
            .iter()
            .find(|track| track.id == old.id)
            .unwrap();
        assert!(std::sync::Arc::ptr_eq(old, kept));
    }
    let inserted = session.project().clone();
    for _ in 0..3 {
        session.undo().unwrap();
    }
    assert_eq!(session.project(), &base);
    for _ in 0..3 {
        session.redo().unwrap();
    }
    assert_eq!(session.project(), &inserted);
    let journal = encode(session.journal());
    assert_eq!(replay(&base, &decode(&journal).unwrap()).unwrap(), inserted);
    let mut actions_only = decode(&journal).unwrap();
    for entry in &mut actions_only {
        entry.changes = None;
    }
    assert_eq!(replay(&base, &actions_only).unwrap(), inserted);
    let directory = std::env::temp_dir().join(format!("echode-track-insertion-{}", Id::generate()));
    write_package(&directory, &inserted).unwrap();
    assert_eq!(read_package(&directory).unwrap(), inserted);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn insertion_rejects_missing_anchors_and_colliding_ids_without_mutation() {
    let (base, anchor) = Project::new().add_track("anchor");
    let mut session = Session::open(base.clone());
    insert(&mut session, anchor, true, "temporary");
    session.undo().unwrap();
    let missing: Command = serde_json::from_value(serde_json::json!({
        "type": "track.insert", "after": Id::generate(), "into_folder": false, "name": "bad",
    }))
    .unwrap();
    assert!(session.perform(missing).is_err());
    assert_eq!(session.project(), &base);
    session.redo().unwrap();
    assert_eq!(session.project().tracks().len(), 2);
    assert_eq!(
        base.insert_track_after_with_id(anchor, anchor, false, "bad", MixerIds::generate()),
        Err(ModelError::DuplicateId)
    );
    let mut mixer = MixerIds::generate();
    mixer.gain = base.tracks()[0].devices[0].parameters[0].id;
    assert_eq!(
        base.insert_track_after_with_id(Id::generate(), anchor, false, "bad", mixer),
        Err(ModelError::DuplicateId)
    );
}

#[test]
fn repeated_insertion_between_the_same_siblings_keeps_the_forest_order() {
    let (project, anchor) = Project::new().add_track("anchor");
    let (mut project, tail) = project.add_track("tail");
    let mut expected = vec![anchor, tail];
    // This is the regression's insertion budget, not a track or key-length limit.
    for _ in 0..128 {
        let id = Id::generate();
        project = project
            .insert_track_after_with_id(id, anchor, true, "inserted", MixerIds::generate())
            .unwrap();
        expected.insert(1, id);
        project.check_invariants().unwrap();
    }
    assert_eq!(
        project
            .tracks()
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>(),
        expected
    );
}
