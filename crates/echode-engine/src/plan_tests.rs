use super::*;
use echode_model::{NoteDraft, Tick};

fn note() -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(0),
        length: Tick::quarters(1).unwrap(),
    }
}

#[test]
fn audit_fca19_rebuilding_keeps_only_current_playback_id_mappings() {
    let (base, kept_track) = Project::new().add_track("kept");
    let (base, _) = base.insert_note(kept_track, note()).unwrap();
    let mut ids = PlaybackIds::new();
    let first = build_plan(&base, base.tempo(), 48_000, &mut ids).unwrap();
    let kept_note_id = first.tracks[0].notes[0].note_id;
    let kept_track_key = first.tracks[0].track_key;
    let mut last_deleted_note_id = kept_note_id;
    // These iterations exercise edit churn only; there is no project-size cap.
    for _ in 0..128 {
        let (with_track, temporary_track) = base.add_track("temporary");
        let (edited, _) = with_track.insert_note(temporary_track, note()).unwrap();
        let added = build_plan(&edited, edited.tempo(), 48_000, &mut ids).unwrap();
        assert_eq!(added.tracks[0].track_key, kept_track_key);
        assert_eq!(added.tracks[0].notes[0].note_id, kept_note_id);
        assert!(added.tracks[1].notes[0].note_id > last_deleted_note_id);
        last_deleted_note_id = added.tracks[1].notes[0].note_id;
        let removed = build_plan(&base, base.tempo(), 48_000, &mut ids).unwrap();
        assert_eq!(removed.tracks[0].notes[0].note_id, kept_note_id);
        assert_eq!(
            ids.notes.len(),
            1,
            "deleted note IDs remain in the playback table"
        );
        assert_eq!(
            ids.tracks.len(),
            1,
            "deleted track IDs remain in the playback table"
        );
    }
    // Old plans may still be in flight, so removed numbers must never be reused.
    let empty = Project::new();
    build_plan(&empty, empty.tempo(), 48_000, &mut ids).unwrap();
    assert_eq!(ids.notes.capacity(), 0);
    assert_eq!(ids.tracks.capacity(), 0);
    let restored = build_plan(&base, base.tempo(), 48_000, &mut ids).unwrap();
    assert!(restored.tracks[0].notes[0].note_id > last_deleted_note_id);
    assert!(restored.tracks[0].track_key > kept_track_key);
    assert_eq!(first.tracks[0].notes[0].note_id, kept_note_id);
}

#[test]
fn a_failed_plan_build_does_not_prune_the_last_successful_ids() {
    let (project, track) = Project::new().add_track("kept");
    let (project, _) = project.insert_note(track, note()).unwrap();
    let mut ids = PlaybackIds::new();
    let first = build_plan(&project, project.tempo(), 48_000, &mut ids).unwrap();
    assert!(build_plan(&project, project.tempo(), 0, &mut ids).is_err());
    let retried = build_plan(&project, project.tempo(), 48_000, &mut ids).unwrap();
    assert_eq!(
        first.tracks[0].notes[0].note_id,
        retried.tracks[0].notes[0].note_id
    );
    assert_eq!(first.tracks[0].track_key, retried.tracks[0].track_key);
}

/// Small deterministic generator, so a failure replays the same edits.
struct Steps(u64);

impl Steps {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

fn edit(project: &Project, steps: &mut Steps) -> Option<Project> {
    use echode_model::{DeviceKind, Id, NoteGeometry, ParamKey, Tempo};
    let tracks = project.tracks();
    let track = &tracks[steps.next(tracks.len())];
    let notes: Vec<_> = track
        .clips
        .iter()
        .flat_map(|clip| {
            project
                .content(clip.content)
                .into_iter()
                .flat_map(|content| &content.notes)
                .map(move |note| (clip.id, note.id, note.start, note.length))
        })
        .collect();
    let picked = notes.get(steps.next(notes.len())).copied();
    let draft = NoteDraft {
        pitch: 48 + steps.next(24) as u8,
        velocity: 100,
        start: Tick::new((steps.next(64) as i64) * 120),
        length: Tick::new(60 + (steps.next(4) as i64) * 60),
    };
    match steps.next(8) {
        0 => Some(project.add_track("added").0),
        1 | 2 => project
            .insert_note(track.id, draft)
            .ok()
            .map(|(next, _)| next),
        3 => picked.and_then(|(_, note, ..)| project.delete_note(track.id, note).ok()),
        4 => picked.and_then(|(clip, note, start, length)| {
            let geometry = NoteGeometry {
                pitch: draft.pitch,
                start,
                length,
            };
            project.edit_note(track.id, clip, note, geometry).ok()
        }),
        5 => {
            let gain = track
                .devices
                .iter()
                .find(|device| device.kind == DeviceKind::Mixer)?
                .parameters
                .iter()
                .find(|parameter| parameter.key == ParamKey::Gain)?
                .id;
            project
                .set_parameter(gain, 0.25 * steps.next(4) as f32)
                .ok()
        }
        6 => Some(project.with_tempo(
            Tick::new(0),
            Tempo::quarters_per_minute(90 + 30 * steps.next(3) as u32)?,
        )),
        _ => {
            // A linked placement puts the same note IDs on two tracks.
            let source = track.clips.get(steps.next(track.clips.len()))?;
            let target = &tracks[steps.next(tracks.len())];
            let start = Tick::new(7_680 * (1 + steps.next(8) as i64));
            project
                .link_clip(source.id, target.id, start, Id::generate())
                .ok()
        }
    }
}

/// A note span's on, off, model ID, pitch and velocity.
type Span = (i64, i64, u128, u8, u16);

/// The comparable content of a track plan, without playback numbers.
fn spans(plan: &TrackPlan) -> (u128, [u32; 4], Vec<Span>, &[u32]) {
    let notes = plan
        .notes
        .iter()
        .map(|note| (note.on, note.off, note.note, note.pitch, note.velocity))
        .collect();
    let mixer = [plan.gain, plan.pan, plan.mute, plan.solo].map(f32::to_bits);
    (plan.track, mixer, notes, &plan.by_off)
}

/// FCA-04: reusing unchanged tracks must give what a full rebuild gives, keep
/// each surviving note's number, and keep the derived lookups coherent.
#[test]
fn reused_track_plans_match_full_rebuilds_across_edits_and_undo() {
    let (mut project, track) = Project::new().add_track("seed");
    for start in [0, 480, 960] {
        let draft = NoteDraft {
            start: Tick::new(start),
            ..note()
        };
        project = project.insert_note(track, draft).unwrap().0;
    }
    let mut history = vec![project];
    let mut steps = Steps(0x9e37_79b9_7f4a_7c15);
    let mut reused = PlaybackIds::new();
    let mut numbers = std::collections::HashMap::<u128, u32>::new();
    let (mut partial, mut full_rebuilds, mut shared) = (0, 0, 0);
    // The step count bounds this regression's run time, not edit history.
    for step in 0..400 {
        let current = history.last().unwrap().clone();
        if steps.next(10) == 0 && history.len() > 1 {
            let keep = 1 + steps.next(history.len() - 1);
            history.truncate(keep);
        } else if let Some(next) = edit(&current, &mut steps) {
            history.push(next);
        }
        let project = history.last().unwrap();
        let built = build(project, project.tempo(), 48_000, &mut reused).unwrap();
        let full = build_plan(project, project.tempo(), 48_000, &mut PlaybackIds::new()).unwrap();
        assert_eq!(built.plan.tracks.len(), full.tracks.len(), "step {step}");
        let mut seen = std::collections::HashMap::<u128, u32>::new();
        for ((planned, rebuilt), derived) in built
            .plan
            .tracks
            .iter()
            .zip(&full.tracks)
            .zip(&built.derived)
        {
            assert_eq!(spans(planned), spans(rebuilt), "step {step}");
            assert_eq!(*derived.index, NoteIndex::new(planned), "step {step}");
            assert_eq!(
                derived.by_number,
                TrackDerived::new(planned).by_number,
                "step {step}"
            );
            for note in &planned.notes {
                // One model note has one number in a plan and keeps it while it plays.
                assert_eq!(*seen.entry(note.note).or_insert(note.note_id), note.note_id);
                if let Some(before) = numbers.get(&note.note) {
                    assert_eq!(
                        *before, note.note_id,
                        "step {step}: a surviving note changed number"
                    );
                }
            }
        }
        assert_eq!(reused.notes.len(), seen.len(), "step {step}");
        let total: usize = full.tracks.iter().map(|track| track.notes.len()).sum();
        if reused.planned_notes < total {
            partial += 1;
        } else if total > 0 {
            full_rebuilds += 1;
        }
        if reused.notes.values().any(|number| number.uses > 1) {
            shared += 1;
        }
        numbers = seen;
    }
    // The edits must reach reuse, full rebuilds (tempo), and shared note IDs.
    assert!(partial > 100, "only {partial} builds reused a track");
    assert!(
        full_rebuilds > 10,
        "only {full_rebuilds} builds replanned every note"
    );
    assert!(
        shared > 10,
        "only {shared} builds shared a note between spans"
    );
}
