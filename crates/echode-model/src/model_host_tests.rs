//! Product entry points share bounded, bidirectional auto-clip host discovery.

use super::*;

pub(super) fn reverse_project(count: usize, wall: bool, shift_bars: i64) -> Project {
    let template = merge_project(count, true);
    let bar = template.meter().bar_ticks().unwrap().get();
    let mut tracks: Vec<_> = template
        .tracks()
        .iter()
        .map(|track| (**track).clone())
        .collect();
    for (index, clip) in tracks[0].clips.iter_mut().take(count).enumerate() {
        clip.start = Tick::new((shift_bars + index as i64) * bar);
        if wall && index == count / 2 {
            clip.kind = ClipKind::Fixed;
        }
    }
    Project::try_assemble(
        template.meter(),
        template.tempo().clone(),
        template
            .contents()
            .iter()
            .map(|content| (**content).clone())
            .collect(),
        tracks,
    )
    .unwrap()
}

#[test]
fn reverse_chain_discovery_is_bounded_from_each_note_edit_entry() {
    // Counts, IDs and timeline offsets constrain these fixtures, not the product.
    for count in [4, 64, 256] {
        for shift in [0, -(count as i64)] {
            let template = reverse_project(count, false, shift);
            let mut contents: Vec<_> = template
                .contents()
                .iter()
                .map(|content| (**content).clone())
                .collect();
            let mut extra = contents[count - 1].notes[0].clone();
            extra.id = Id::from_raw(8_000_000);
            extra.start = Tick::new(1);
            contents[count - 1]
                .notes
                .edit(|notes| notes.push(extra.clone()));
            let project = Project::try_assemble(
                template.meter(),
                template.tempo().clone(),
                contents,
                template
                    .tracks()
                    .iter()
                    .map(|track| (**track).clone())
                    .collect(),
            )
            .unwrap();
            let original = project.clone();
            let owner = &project.tracks()[0];
            let clip = &owner.clips[count - 1];
            let note = &project.content(clip.content).unwrap().notes[0];
            let start = clip.sounding_start(note).unwrap();
            for operation in ["insert", "edit", "duplicate", "delete", "linked edit"] {
                let (edited, work) = measured(|| match operation {
                    "insert" => project.insert_note_with_id(
                        owner.id,
                        Id::from_raw(3_000_000),
                        Id::from_raw(3_000_001),
                        Id::from_raw(3_000_002),
                        NoteDraft {
                            pitch: 61,
                            velocity: 100,
                            start,
                            length: Tick::new(1),
                        },
                    ),
                    "duplicate" => project.duplicate_note(
                        owner.id,
                        clip.id,
                        note.id,
                        Id::from_raw(3_000_000),
                        61,
                        start,
                    ),
                    "delete" => project.delete_note(owner.id, extra.id),
                    _ => {
                        let (track, clip) = if operation == "linked edit" {
                            (&project.tracks()[1], &project.tracks()[1].clips[0])
                        } else {
                            (owner, clip)
                        };
                        project.edit_note(
                            track.id,
                            clip.id,
                            note.id,
                            NoteGeometry {
                                pitch: 61,
                                start: clip.sounding_start(note).unwrap(),
                                length: note.length,
                            },
                        )
                    }
                });
                let edited = edited.unwrap();
                assert!(
                    work.refit_host_probes <= 2 * count,
                    "reverse chain searched {} host indices for {count} autos ({operation}, shift={shift})",
                    work.refit_host_probes
                );
                edited.check_invariants().unwrap();
                assert!(project.diff(&original).is_empty());
                let primary = &edited.tracks()[0].clips[0];
                let bar = project.meter().bar_ticks().unwrap().get();
                assert_eq!(primary.id, owner.clips[0].id);
                assert_eq!(primary.start, owner.clips[0].start);
                assert_eq!(primary.length, Tick::new(count as i64 * bar));
                assert_eq!(edited.tracks()[0].clips.len(), count + 1);
                let merged = edited.content(primary.content).unwrap();
                let expected_count = match operation {
                    "insert" | "duplicate" => count + 2,
                    "delete" => count,
                    _ => count + 1,
                };
                assert_eq!(merged.notes.len(), expected_count);
                for index in 0..count {
                    let old = &project.contents()[index].notes[0];
                    let new = merged.notes.iter().find(|note| note.id == old.id).unwrap();
                    let mut expected = old.clone();
                    expected.start = Tick::new(index as i64 * bar);
                    if index == count - 1 && matches!(operation, "edit" | "linked edit") {
                        expected.pitch = 61;
                    }
                    assert_eq!(*new, expected);
                }
                for (before, after) in project.tracks()[..3].iter().zip(&edited.tracks()[..3]) {
                    for old in before
                        .clips
                        .iter()
                        .filter(|clip| clip.kind == ClipKind::Fixed)
                    {
                        let new = after.clips.iter().find(|clip| clip.id == old.id).unwrap();
                        let old_note = &project.content(old.content).unwrap().notes[0];
                        let new_note = merged
                            .notes
                            .iter()
                            .find(|note| note.id == old_note.id)
                            .unwrap();
                        let mut expected = old.clone();
                        expected.content = primary.content;
                        expected.offset = Tick::new(old.offset.get() + new_note.start.get());
                        assert_eq!(*new, expected);
                        assert_eq!(old.sounding_start(old_note), new.sounding_start(new_note));
                    }
                }
                assert!(Arc::ptr_eq(&project.tracks()[3], &edited.tracks()[3]));
                assert!(Arc::ptr_eq(
                    project.contents().last().unwrap(),
                    edited.contents().last().unwrap()
                ));
                let changes = project.diff(&edited);
                assert!(
                    project
                        .apply_changeset(&changes)
                        .unwrap()
                        .diff(&edited)
                        .is_empty()
                );
                assert!(
                    edited
                        .apply_changeset(&changes.invert())
                        .unwrap()
                        .diff(&project)
                        .is_empty()
                );
            }
        }
    }
}
