use echode_model::{Id, NoteDraft, NoteGeometry, Project, Tick};

fn draft(start: i64, length: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::quarters(start).unwrap(),
        length: Tick::quarters(length).unwrap(),
    }
}

#[test]
fn editing_keeps_identity_and_refits_and_sorts_auto_notes() {
    let (project, track) = Project::new().add_track("keys");
    let (project, note) = project.insert_note(track, draft(0, 1)).unwrap();
    let (project, other) = project.insert_note(track, draft(2, 1)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let before = project.contents()[0].notes[0].clone();
    let edited = project
        .edit_note(
            track,
            clip,
            note,
            NoteGeometry {
                pitch: 67,
                start: Tick::quarters(6).unwrap(),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap();
    edited.check_invariants().unwrap();
    let notes = &edited.contents()[0].notes;
    assert_eq!(notes[0].id, other);
    assert_eq!(notes[1].id, note);
    assert_eq!(notes[1].pitch, 67);
    assert_eq!(notes[1].velocity, before.velocity);
    assert_eq!(notes[1].release, before.release);
    assert_eq!(
        edited.tracks()[0].clips[0].length,
        Tick::quarters(8).unwrap()
    );
    assert_eq!(project.contents()[0].notes[0], before);
}

#[test]
fn extending_into_another_auto_clip_merges_without_replacing_notes() {
    let (project, track) = Project::new().add_track("keys");
    let (project, note) = project.insert_note(track, draft(0, 1)).unwrap();
    let (project, other) = project.insert_note(track, draft(8, 1)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let edited = project
        .edit_note(
            track,
            clip,
            note,
            NoteGeometry {
                pitch: 60,
                start: Tick::quarters(0).unwrap(),
                length: Tick::quarters(9).unwrap(),
            },
        )
        .unwrap();
    edited.check_invariants().unwrap();
    assert_eq!(edited.tracks()[0].clips.len(), 1);
    assert!(
        edited.contents()[0]
            .notes
            .iter()
            .any(|item| item.id == other)
    );
    assert!(
        edited.contents()[0]
            .notes
            .iter()
            .any(|item| item.id == note)
    );
}

#[test]
fn linked_windows_keep_their_sounding_times_when_contents_merge() {
    let (project, track) = Project::new().add_track("keys");
    let (project, dest) = project.add_track("linked");
    let (project, _) = project.insert_note(track, draft(0, 1)).unwrap();
    let (project, note) = project.insert_note(track, draft(8, 1)).unwrap();
    let source = project.tracks()[0].clips[1].id;
    let linked = Id::generate();
    let project = project
        .link_clip(source, dest, Tick::quarters(32).unwrap(), linked)
        .unwrap();
    let edited = project.insert_note(track, draft(1, 8)).unwrap().0;
    edited.check_invariants().unwrap();
    let window = edited.tracks()[1]
        .clips
        .iter()
        .find(|item| item.id == linked)
        .unwrap();
    let stored = edited
        .content(window.content)
        .unwrap()
        .notes
        .iter()
        .find(|item| item.id == note)
        .unwrap();
    assert_eq!(
        window.sounding_start(stored),
        Some(Tick::quarters(32).unwrap())
    );
    let edited = edited
        .edit_note(
            dest,
            linked,
            note,
            NoteGeometry {
                pitch: 63,
                start: Tick::quarters(33).unwrap(),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap();
    edited.check_invariants().unwrap();
    let window = &edited.tracks()[1].clips[0];
    let stored = edited
        .content(window.content)
        .unwrap()
        .notes
        .iter()
        .find(|item| item.id == note)
        .unwrap();
    assert_eq!(
        window.sounding_start(stored),
        Some(Tick::quarters(33).unwrap())
    );
    assert_eq!(window.start, Tick::quarters(32).unwrap());
}

#[test]
fn editing_through_a_fixed_link_can_merge_the_auto_owner_on_another_track() {
    let (project, track) = Project::new().add_track("keys");
    let (project, dest) = project.add_track("linked");
    let (project, _) = project.insert_note(track, draft(0, 1)).unwrap();
    let (project, note) = project.insert_note(track, draft(8, 1)).unwrap();
    let source = project.tracks()[0].clips[1].id;
    let linked = Id::generate();
    let project = project
        .link_clip(source, dest, Tick::quarters(32).unwrap(), linked)
        .unwrap();
    let edited = project
        .edit_note(
            dest,
            linked,
            note,
            NoteGeometry {
                pitch: 62,
                start: Tick::quarters(25).unwrap(),
                length: Tick::quarters(8).unwrap(),
            },
        )
        .unwrap();
    edited.check_invariants().unwrap();
    assert_eq!(edited.tracks()[0].clips.len(), 1);
    let window = &edited.tracks()[1].clips[0];
    assert_eq!(window.id, linked);
    assert_eq!(window.start, Tick::quarters(32).unwrap());
    let stored = edited
        .content(window.content)
        .unwrap()
        .notes
        .iter()
        .find(|item| item.id == note)
        .unwrap();
    assert_eq!(
        window.sounding_start(stored),
        Some(Tick::quarters(25).unwrap())
    );
}

#[test]
fn editing_an_auto_across_a_fixed_window_fails_without_changing_the_snapshot() {
    let (project, track) = Project::new().add_track("keys");
    let (project, _) = project.insert_note(track, draft(0, 1)).unwrap();
    let (project, note) = project.insert_note(track, draft(8, 1)).unwrap();
    let fixed = project.tracks()[0].clips[0].id;
    let auto = project.tracks()[0].clips[1].id;
    let project = project
        .resize_clip(
            track,
            fixed,
            Tick::quarters(4).unwrap(),
            Tick::quarters(4).unwrap(),
        )
        .unwrap();
    assert!(
        project
            .edit_note(
                track,
                auto,
                note,
                NoteGeometry {
                    pitch: 62,
                    start: Tick::quarters(2).unwrap(),
                    length: Tick::quarters(8).unwrap(),
                }
            )
            .is_err()
    );
    project.check_invariants().unwrap();
    assert_eq!(project.tracks()[0].clips.len(), 2);
}
