use std::collections::HashSet;
use std::sync::Arc;

use echode_actions::{ActionError, Command, Session, decode, encode, replay};
use echode_history::{read_package, write_object, write_package};
use echode_model::{
    AudioRef, ClipKind, Content, Id, ModelError, NoteDraft, Project, TICKS_PER_QUARTER, Tick, Track,
};

const STATE: &[u8] = b"echode-duplicate-state";
const BAR: i64 = TICKS_PER_QUARTER * 4;
const AUDIO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Fixture {
    project: Project,
    folder: Id,
    kick: Id,
    layer: Id,
    snare: Id,
    other: Id,
    tail: Id,
}

fn track(project: &Project, id: Id) -> &Arc<Track> {
    project
        .tracks()
        .iter()
        .find(|track| track.id == id)
        .unwrap()
}

fn content(project: &Project, id: Id) -> &Content {
    project.content(id).unwrap()
}

fn ids(project: &Project) -> Vec<Id> {
    project.tracks().iter().map(|track| track.id).collect()
}

/// A folder with a plugin chain, a nested subtree, a clip linked inside and
/// outside the subtree, audio, and mixer values away from their defaults.
fn fixture(state: &str) -> Fixture {
    let (project, folder) = Project::new().add_track("drums");
    let (project, kick) = project.add_child(folder, "kick").unwrap();
    let (project, layer) = project.add_child(kick, "kick layer").unwrap();
    let (project, snare) = project.add_child(folder, "snare").unwrap();
    let (project, other) = project.add_track("other");
    let (project, tail) = project.add_track("tail");
    let (fx, offline) = (Id::generate(), Id::generate());
    let mut project = project
        .add_clap_device(folder, fx, "org.example.fx")
        .unwrap()
        .set_device_state(fx, Some(state.to_string()))
        .unwrap()
        .set_device_bypassed(fx, true)
        .unwrap()
        .add_clap_device(folder, offline, "org.example.missing")
        .unwrap()
        .set_device_offline(offline, true)
        .unwrap();
    let mixer = track(&project, folder).devices.last().unwrap().clone();
    for (parameter, value) in mixer.parameters.iter().zip([0.5, -0.25, 1.0, 0.0]) {
        project = project.set_parameter(parameter.id, value).unwrap();
    }
    for (pitch, start) in [(36, 0), (36, TICKS_PER_QUARTER), (38, TICKS_PER_QUARTER)] {
        let draft = NoteDraft {
            pitch,
            velocity: 100,
            start: Tick::new(start),
            length: Tick::new(TICKS_PER_QUARTER / 2),
        };
        project = project.insert_note(kick, draft).unwrap().0;
    }
    let auto = track(&project, kick).clips[0].id;
    let project = project
        .link_clip(auto, kick, Tick::new(BAR * 4), Id::generate())
        .unwrap()
        .link_clip(auto, other, Tick::new(BAR * 2), Id::generate())
        .unwrap();
    let audio = AudioRef::try_new(AUDIO.to_string(), 48_000, 1, 48_000).unwrap();
    let project = project
        .place_audio(layer, Id::generate(), Id::generate(), Tick::new(BAR), audio)
        .unwrap();
    project.check_invariants().unwrap();
    Fixture {
        project,
        folder,
        kick,
        layer,
        snare,
        other,
        tail,
    }
}

fn duplicate(session: &mut Session, source: Id) -> Vec<Id> {
    let before: HashSet<Id> = ids(session.project()).into_iter().collect();
    let command: Command =
        serde_json::from_value(serde_json::json!({ "type": "track.duplicate", "source": source }))
            .expect("the product accepts track duplication");
    session.perform(command).unwrap();
    session.project().check_invariants().unwrap();
    ids(session.project())
        .into_iter()
        .filter(|id| !before.contains(id))
        .collect()
}

fn assert_same_track_values(project: &Project, original: Id, copy: Id) {
    let (original, copy) = (track(project, original), track(project, copy));
    assert_eq!(copy.name, original.name);
    assert_eq!(copy.devices.len(), original.devices.len());
    for (from, to) in original.devices.iter().zip(&copy.devices) {
        assert_ne!(to.id, from.id);
        assert_eq!(
            (to.kind, &to.plugin, &to.state, to.bypassed, to.offline),
            (
                from.kind,
                &from.plugin,
                &from.state,
                from.bypassed,
                from.offline
            )
        );
        for (from, to) in from.parameters.iter().zip(&to.parameters) {
            assert_ne!(to.id, from.id);
            assert_eq!(
                (to.key, to.value.to_bits()),
                (from.key, from.value.to_bits())
            );
        }
    }
    assert_eq!(copy.clips.len(), original.clips.len());
    for (from, to) in original.clips.iter().zip(&copy.clips) {
        assert_ne!(to.id, from.id);
        assert_ne!(to.content, from.content, "a normal copy owns its content");
        assert_eq!(
            (to.start, to.length, to.offset, to.kind, to.gain.to_bits()),
            (
                from.start,
                from.length,
                from.offset,
                from.kind,
                from.gain.to_bits()
            )
        );
        assert_eq!((to.fade_in, to.fade_out), (from.fade_in, from.fade_out));
        let (from, to) = (content(project, from.content), content(project, to.content));
        assert_eq!(to.audio, from.audio);
        assert_eq!(to.notes.len(), from.notes.len());
        let geometry = |content: &Content| -> Vec<_> {
            let mut notes: Vec<_> = content
                .notes
                .iter()
                .map(|note| (note.start, note.length, note.pitch, note.velocity))
                .collect();
            notes.sort();
            notes
        };
        assert_eq!(geometry(to), geometry(from));
        assert!(
            to.notes
                .iter()
                .all(|note| from.notes.iter().all(|old| old.id != note.id))
        );
    }
}

#[test]
fn folder_copy_keeps_its_subtree_chain_and_clips_and_survives_history_and_reopen() {
    let directory = std::env::temp_dir().join(format!("echode-track-duplicate-{}", Id::generate()));
    let state = write_object(&directory, STATE).unwrap();
    let Fixture {
        project: base,
        folder,
        kick,
        layer,
        snare,
        other,
        tail,
    } = fixture(&state);
    let mut session = Session::open(base.clone());
    let copies = duplicate(&mut session, folder);
    let project = session.project().clone();
    let [folder_copy, kick_copy, layer_copy, snare_copy] = copies[..] else {
        panic!("the folder copy has four tracks, got {copies:?}");
    };
    assert_eq!(
        ids(&project),
        [
            folder,
            kick,
            layer,
            snare,
            folder_copy,
            kick_copy,
            layer_copy,
            snare_copy,
            other,
            tail
        ]
    );
    assert_eq!(project.parent(folder_copy).unwrap(), None);
    assert_eq!(project.parent(kick_copy).unwrap(), Some(folder_copy));
    assert_eq!(project.parent(layer_copy).unwrap(), Some(kick_copy));
    assert_eq!(project.parent(snare_copy).unwrap(), Some(folder_copy));
    for (original, copy) in [
        (folder, folder_copy),
        (kick, kick_copy),
        (layer, layer_copy),
        (snare, snare_copy),
    ] {
        assert_same_track_values(&project, original, copy);
    }
    let kick_clips = &track(&project, kick_copy).clips;
    assert_eq!(kick_clips[0].kind, ClipKind::Auto);
    assert_eq!(
        kick_clips[0].content, kick_clips[1].content,
        "clips linked inside the copy still share one content"
    );
    let shared = track(&project, kick).clips[0].content;
    assert_eq!(track(&project, other).clips[0].content, shared);
    for old in base.tracks() {
        assert!(Arc::ptr_eq(old, track(&project, old.id)));
    }

    let note = content(&project, kick_clips[0].content).notes[0].clone();
    session
        .perform(Command::EditNote {
            track: kick_copy,
            clip: kick_clips[0].id,
            note: note.id,
            pitch: 49,
            start: note.start,
            length: note.length,
        })
        .unwrap();
    assert_eq!(content(session.project(), shared), content(&base, shared));
    session.undo().unwrap();

    session.undo().unwrap();
    assert_eq!(session.project(), &base);
    session.redo().unwrap();
    assert_eq!(session.project(), &project);
    let journal = encode(session.journal());
    assert_eq!(replay(&base, &decode(&journal).unwrap()).unwrap(), project);
    let mut actions_only = decode(&journal).unwrap();
    for entry in &mut actions_only {
        entry.changes = None;
    }
    assert_eq!(replay(&base, &actions_only).unwrap(), project);
    write_package(&directory, &project).unwrap();
    assert_eq!(read_package(&directory).unwrap(), project);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn child_copy_lands_after_its_subtree_inside_the_same_folder() {
    let Fixture {
        project,
        folder,
        kick,
        layer,
        snare,
        other,
        tail,
    } = fixture(&"0".repeat(64));
    let seed = Id::generate();
    let copied = project.duplicate_track(kick, seed).unwrap();
    copied.check_invariants().unwrap();
    assert_eq!(copied, project.duplicate_track(kick, seed).unwrap());
    let added: Vec<Id> = ids(&copied)
        .into_iter()
        .filter(|id| !ids(&project).contains(id))
        .collect();
    let [kick_copy, layer_copy] = added[..] else {
        panic!("the child copy has two tracks, got {added:?}");
    };
    assert_eq!(
        ids(&copied),
        [
            folder, kick, layer, kick_copy, layer_copy, snare, other, tail
        ]
    );
    assert_eq!(copied.parent(kick_copy).unwrap(), Some(folder));
    assert_eq!(copied.parent(layer_copy).unwrap(), Some(kick_copy));
    // A second copy of the same source lands between the source and the first copy.
    let again = copied.duplicate_track(kick, Id::generate()).unwrap();
    again.check_invariants().unwrap();
    let order = ids(&again);
    assert_eq!(order[..3], [folder, kick, layer]);
    assert_eq!(order[5..], [kick_copy, layer_copy, snare, other, tail]);
    assert_eq!(again.parent(order[3]).unwrap(), Some(folder));
    assert_eq!(again.parent(order[4]).unwrap(), Some(order[3]));
}

#[test]
fn refused_copies_leave_the_project_and_history_unchanged() {
    let Fixture {
        project: base,
        kick,
        ..
    } = fixture(&"0".repeat(64));
    let seed = Id::generate();
    let copied = base.duplicate_track(kick, seed).unwrap();
    assert_eq!(
        copied.duplicate_track(kick, seed),
        Err(ModelError::DuplicateId),
        "a reused seed collides with the earlier copy"
    );
    assert_eq!(
        base.duplicate_track(Id::generate(), seed),
        Err(ModelError::MissingTrack)
    );
    let mut session = Session::open(base.clone());
    let missing: Command = serde_json::from_value(
        serde_json::json!({ "type": "track.duplicate", "source": Id::generate() }),
    )
    .unwrap();
    assert_eq!(
        session.perform(missing),
        Err(ActionError::Model(ModelError::MissingTrack))
    );
    assert_eq!(session.project(), &base);
    assert!(session.journal().is_empty());
}
