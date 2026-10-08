//! Open items from docs/reviews/2026-10-08-audit.md. Each test fails until its item
//! is fixed; `make known-bugs` runs them. Remove the `#[ignore]` with the fix.

use echode_model::{Id, NoteDraft, Project, Tick};

/// Every child must sit after its parent, inside the parent's contiguous run of descendants.
fn assert_children_follow_their_parent(project: &Project) {
    let tracks = project.tracks();
    let position = |id: Id| tracks.iter().position(|track| track.id == id).unwrap();
    for (index, track) in tracks.iter().enumerate() {
        let Some(parent) = track.parent else {
            continue;
        };
        let start = position(parent);
        assert!(start < index, "{} is above its parent", track.name);
        for between in &tracks[start + 1..index] {
            let mut cursor = between.parent;
            let mut inside = false;
            while let Some(id) = cursor {
                if id == parent {
                    inside = true;
                    break;
                }
                cursor = tracks[position(id)].parent;
            }
            assert!(
                inside,
                "{} sits between {} and its parent",
                between.name, track.name
            );
        }
    }
}

#[test]
#[ignore = "MOD-3: set_parent leaves a child above its new parent"]
fn audit_mod3_children_follow_their_parent() {
    let (project, a) = Project::new().add_track("A");
    let (project, b) = project.add_track("B");
    let (project, _) = project.add_child(a, "a1").unwrap();
    let project = project.set_parent(a, Some(b)).unwrap();
    assert_children_follow_their_parent(&project);
}

fn quarter_note(start: i64, length: i64) -> NoteDraft {
    NoteDraft {
        pitch: 64,
        velocity: 100,
        start: Tick::quarters(start).unwrap(),
        length: Tick::quarters(length).unwrap(),
    }
}

#[test]
#[ignore = "MOD-6: a long note that starts inside an auto clip is refused instead of merged"]
fn audit_mod6_a_long_note_merges_the_same_way_wherever_it_starts() {
    let clips_after = |start: i64, length: i64| {
        let (project, track) = Project::new().add_track("piano");
        let (project, _) = project.insert_note(track, quarter_note(0, 1)).unwrap();
        let (project, _) = project.insert_note(track, quarter_note(8, 1)).unwrap();
        project
            .insert_note(track, quarter_note(start, length))
            .map(|(project, _)| project.tracks()[0].clips.len())
    };
    // Both notes end in bar 2. The first starts inside bar 0's clip, the second in the gap.
    assert_eq!(clips_after(1, 8), clips_after(4, 5));
}
