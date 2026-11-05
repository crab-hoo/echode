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
