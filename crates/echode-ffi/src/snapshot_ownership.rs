use super::*;
use echode_model::{Id, MixerIds};
use std::sync::Arc;

fn snapshot() -> *mut Snapshot {
    let project = Project::new()
        .add_track_with_id(Id::from_raw(1), "鍵盤 🎹", MixerIds::from_base(16))
        .unwrap();
    Box::into_raw(Box::new(Snapshot::from_project(Arc::new(project), None)))
}

#[test]
fn contract_arrange_projection_retains_hierarchy_rows_and_controls_after_source_release() {
    // Deterministic identities keep the ownership fixture independent of OS
    // clock/randomness under Miri. These IDs only describe this four-row tree.
    let folder = Id::from_raw(1);
    let child = Id::from_raw(2);
    let tail = Id::from_raw(4);
    let project = Project::new()
        .add_track_with_id(folder, "folder", MixerIds::from_base(16))
        .unwrap()
        .add_child_with_id(child, folder, "child", MixerIds::from_base(32))
        .unwrap()
        .add_child_with_id(Id::from_raw(3), child, "nested", MixerIds::from_base(48))
        .unwrap()
        .add_track_with_id(tail, "tail", MixerIds::from_base(64))
        .unwrap();
    let gain = project.tracks()[3].devices[0].parameters[0].id;
    let project = Arc::new(project.set_parameter(gain, 0.25).unwrap());
    let source = Box::into_raw(Box::new(Snapshot::from_project(Arc::clone(&project), None)));
    let collapsed = *uuid::Uuid::from_u128(folder.raw()).as_bytes();
    // SAFETY: `source` is owned; a null UUID array with a count is refused.
    assert!(unsafe { echode_snapshot_project_arrange(source, std::ptr::null(), 1) }.is_null());
    assert!(
        // SAFETY: `source` is owned and `collapsed` holds one UUID; a count beyond `isize::MAX`
        // bytes is refused.
        unsafe { echode_snapshot_project_arrange(source, collapsed.as_ptr(), usize::MAX) }
            .is_null()
    );
    assert!(
        // SAFETY: null is an accepted snapshot and `collapsed` holds one UUID.
        unsafe { echode_snapshot_project_arrange(std::ptr::null(), collapsed.as_ptr(), 1) }
            .is_null()
    );
    // SAFETY: `source` is owned and `collapsed` holds one UUID.
    let projected = unsafe { echode_snapshot_project_arrange(source, collapsed.as_ptr(), 1) };
    assert!(!projected.is_null());
    // SAFETY: `source` is owned and not yet freed.
    assert_eq!(unsafe { echode_snapshot_track_count(source) }, 4);
    // SAFETY: `projected` is owned and not yet freed.
    assert_eq!(unsafe { echode_snapshot_track_count(projected) }, 2);
    // SAFETY: `projected` is owned and not yet freed.
    let retained = unsafe { echode_snapshot_retain(projected) };
    // SAFETY: both handles are owned and each is freed once; `retained` keeps the projection.
    unsafe {
        echode_snapshot_free(projected);
        echode_snapshot_free(source);
    }
    let mut tree = TrackTreeInfo::default();
    assert_eq!(
        // SAFETY: `retained` is owned and `tree` is writable.
        unsafe { echode_snapshot_track_tree(retained, 0, &mut tree) },
        0
    );
    assert_eq!(tree.has_children, 1);
    assert_eq!(tree.has_parent, 0);
    assert_eq!(
        // SAFETY: `retained` is owned and `tree` is writable.
        unsafe { echode_snapshot_track_tree(retained, 1, &mut tree) },
        0
    );
    assert_eq!(tree.depth, 0);
    let mut id = [0; 16]; // One UUID, for this ABI fixture only.
    assert_eq!(
        // SAFETY: `retained` is owned and `id` has 16 writable bytes.
        unsafe { echode_snapshot_track_id(retained, 1, id.as_mut_ptr()) },
        0
    );
    assert_eq!(u128::from_be_bytes(id), tail.raw());
    let mut mixer = MixerControls::default();
    assert_eq!(
        // SAFETY: `retained` is owned and `mixer` is writable.
        unsafe { echode_snapshot_mixer_controls(retained, 1, &mut mixer) },
        0
    );
    assert_eq!(mixer.gain, 0.25);
    assert_eq!(
        // SAFETY: `retained` is owned and `mixer` is writable.
        unsafe { echode_snapshot_mixer_controls(retained, 2, &mut mixer) },
        ECHODE_ERR
    );
    let mut written = 0;
    let mut dropped = 0;
    // Two rows and one canary are this fixture's query capacity, not a product limit.
    let canary = VisibleElement {
        role: u32::MAX,
        track_id: [0; 16],
        placement_id: [0; 16],
        note_id: [0; 16],
        has_placement: 0,
        has_note: 0,
        pad: [0; 2],
        x: 0.,
        y: 0.,
        w: 0.,
        h: 0.,
    };
    let mut elements = [canary; 3];
    assert_eq!(
        // SAFETY: `retained` is owned, `elements` has room for the two requested elements and the
        // counts are separate locals.
        unsafe {
            echode_snapshot_visible(
                retained,
                0,
                705_600,
                0.5,
                1.5,
                1.,
                64.,
                elements.as_mut_ptr(),
                2,
                &mut written,
                &mut dropped,
            )
        },
        0
    );
    assert_eq!((written, dropped), (2, 0));
    assert_eq!(elements[0].y, -32.);
    assert_eq!(elements[1].y, 32.);
    assert_eq!(u128::from_be_bytes(elements[1].track_id), tail.raw());
    assert_eq!(elements[2].role, u32::MAX);
    // SAFETY: `retained` is owned; a null UUID array has a zero count.
    let expanded = unsafe { echode_snapshot_project_arrange(retained, std::ptr::null(), 0) };
    // SAFETY: `retained` is owned and freed once; `expanded` is a separate handle.
    unsafe { echode_snapshot_free(retained) };
    // SAFETY: `expanded` is owned and not yet freed.
    assert_eq!(unsafe { echode_snapshot_track_count(expanded) }, 4);
    assert_eq!(
        // SAFETY: `expanded` is owned and `tree` is writable.
        unsafe { echode_snapshot_track_tree(expanded, 2, &mut tree) },
        0
    );
    assert_eq!(tree.depth, 2);
    assert_eq!(u128::from_be_bytes(tree.parent), child.raw());
    assert_eq!(
        // SAFETY: `expanded` is owned and `tree` is writable.
        unsafe { echode_snapshot_track_tree(expanded, 4, &mut tree) },
        -1
    );
    assert_eq!(
        // SAFETY: `expanded` is owned; a null `out` is refused.
        unsafe { echode_snapshot_track_tree(expanded, 0, std::ptr::null_mut()) },
        -1
    );
    // SAFETY: `expanded` is owned and freed once.
    unsafe { echode_snapshot_free(expanded) };
    assert_eq!(project.tracks().len(), 4);
    project.check_invariants().unwrap();
}

#[test]
fn retained_snapshot_keeps_borrowed_utf8_and_uuid_alive() {
    let original = snapshot();
    // SAFETY: `original` is owned and not yet freed.
    let retained = unsafe { echode_snapshot_retain(original) };
    let mut length = 0;
    // SAFETY: `retained` is owned and `length` is writable.
    let name = unsafe { echode_snapshot_track_name(retained, 0, &mut length) };
    // SAFETY: `original` is owned and freed once; `retained` is a separate handle.
    unsafe { echode_snapshot_free(original) };
    assert_eq!(
        // SAFETY: `retained` still lends `length` bytes at `name`, though `original` was freed.
        unsafe { std::slice::from_raw_parts(name, length) },
        "鍵盤 🎹".as_bytes()
    );
    let mut bytes = [0; 16];
    assert_eq!(
        // SAFETY: `retained` is owned and `bytes` has 16 writable bytes.
        unsafe { echode_snapshot_track_id(retained, 0, bytes.as_mut_ptr()) },
        0
    );
    assert_eq!(uuid::Uuid::from_bytes(bytes).as_u128(), 1);
    // SAFETY: `retained` is owned and `length` is writable.
    assert!(unsafe { echode_snapshot_track_name(retained, 1, &mut length) }.is_null());
    assert_eq!(length, 0);
    // SAFETY: `retained` is owned and freed once.
    unsafe { echode_snapshot_free(retained) };
}

#[test]
fn retained_mix_controls_copy_ids_and_resolve_fades_across_tempo_changes() {
    use echode_model::{AudioRef, FLICKS_PER_SECOND, ParamKey, Tempo, Tick};
    let track = Id::from_raw(1);
    let clip = Id::from_raw(32);
    let project = Project::new()
        .add_track_with_id(track, "audio", MixerIds::from_base(16))
        .unwrap()
        .with_tempo(
            Tick::quarters(2).unwrap(),
            Tempo::quarters_per_minute(60).unwrap(),
        )
        .place_audio(
            track,
            clip,
            Id::from_raw(33),
            Tick::new(0),
            AudioRef::try_new("a".repeat(64), 48_000, 2, 144_000).unwrap(),
        )
        .unwrap()
        .set_clip_gain(clip, 0.5)
        .unwrap()
        .set_clip_fades(clip, FLICKS_PER_SECOND * 3 / 2, FLICKS_PER_SECOND * 3 / 2)
        .unwrap();
    let gain = project.tracks()[0].devices[0]
        .parameters
        .iter()
        .find(|parameter| parameter.key == ParamKey::Gain)
        .unwrap()
        .id;
    let project = project.set_parameter(gain, 0.25).unwrap();
    let plugin = Id::from_raw(48);
    let project = project
        .add_clap_device(track, plugin, "org.example.offline")
        .unwrap()
        .set_device_offline(plugin, true)
        .unwrap()
        .set_parameter_pinned(plugin, 12, true)
        .unwrap()
        .set_parameter_pinned(plugin, 3, true)
        .unwrap();
    let original = Box::into_raw(Box::new(Snapshot::from_project(Arc::new(project), None)));
    // SAFETY: `original` is owned and not yet freed.
    let retained = unsafe { echode_snapshot_retain(original) };
    // SAFETY: `original` is owned and freed once; `retained` is a separate handle.
    unsafe { echode_snapshot_free(original) };
    // SAFETY: `retained` is owned and not yet freed.
    assert_eq!(unsafe { echode_snapshot_device_offline(retained, 0, 0) }, 1);
    // SAFETY: `retained` is owned and not yet freed.
    assert_eq!(unsafe { echode_snapshot_device_offline(retained, 0, 1) }, 0);
    assert_eq!(
        // SAFETY: `retained` is owned and not yet freed.
        unsafe { echode_snapshot_device_offline(retained, 1, 0) },
        -1
    );
    assert_eq!(
        // SAFETY: `retained` is owned and not yet freed.
        unsafe { echode_snapshot_device_offline(retained, 0, usize::MAX) },
        -1
    );
    assert_eq!(
        // SAFETY: null is an accepted snapshot.
        unsafe { echode_snapshot_device_offline(std::ptr::null(), 0, 0) },
        -1
    );
    let pins = |track, device| {
        let mut count = usize::MAX;
        // SAFETY: `retained` is owned and `count` is writable.
        let pointer = unsafe { echode_snapshot_device_pins(retained, track, device, &mut count) };
        // SAFETY: `retained` lends `count` values at a non-null `pointer` until it is freed.
        (!pointer.is_null()).then(|| unsafe { std::slice::from_raw_parts(pointer, count) }.to_vec())
    };
    assert_eq!(pins(0, 0), Some(vec![3, 12]));
    assert_eq!(pins(0, 1), Some(Vec::new()));
    assert_eq!(pins(1, 0), None);
    assert_eq!(pins(0, usize::MAX), None);
    // SAFETY: `retained` is owned; a null count is refused.
    assert!(unsafe { echode_snapshot_device_pins(retained, 0, 0, std::ptr::null_mut()) }.is_null());
    let mut mixer = MixerControls::default();
    assert_eq!(
        // SAFETY: `retained` is owned and `mixer` is writable.
        unsafe { echode_snapshot_mixer_controls(retained, 0, &mut mixer) },
        ECHODE_OK
    );
    assert_eq!(u128::from_be_bytes(mixer.gain_id), gain.raw());
    assert_eq!(mixer.gain, 0.25);
    assert_eq!(
        // SAFETY: `retained` is owned and `mixer` is writable.
        unsafe { echode_snapshot_mixer_controls(retained, 1, &mut mixer) },
        ECHODE_ERR
    );
    assert_eq!(
        // SAFETY: `retained` is owned; a null `out` is refused.
        unsafe { echode_snapshot_mixer_controls(retained, 0, std::ptr::null_mut()) },
        ECHODE_ERR
    );
    // This byte array tests an unaligned UUID input, not a product limit.
    let mut unaligned = [0u8; 17];
    unaligned[1..].copy_from_slice(&clip.raw().to_be_bytes());
    let mut value = ClipControls::default();
    assert_eq!(
        // SAFETY: `retained` is owned, the UUID has 16 readable bytes and `value` is writable.
        unsafe { echode_snapshot_clip_controls(retained, unaligned[1..].as_ptr(), &mut value) },
        ECHODE_OK
    );
    assert_eq!(u128::from_be_bytes(value.track_id), track.raw());
    assert_eq!(value.gain, 0.5);
    assert_eq!(value.audio, 1);
    assert_eq!(value.length, 705_600 * 4);
    assert_eq!(value.fade_in_ticks, 705_600 * 5 / 2);
    assert_eq!(value.fade_out_ticks, 705_600 * 3 / 2);
    let mut time = 0;
    assert_eq!(
        // SAFETY: `retained` is owned and `time` is writable.
        unsafe { echode_snapshot_flicks(retained, value.fade_in_ticks, &mut time) },
        ECHODE_OK
    );
    assert_eq!(time, FLICKS_PER_SECOND * 3 / 2);
    assert_eq!(
        // SAFETY: `retained` is owned; a null UUID is refused.
        unsafe { echode_snapshot_clip_controls(retained, std::ptr::null(), &mut value) },
        ECHODE_ERR
    );
    assert_eq!(
        // SAFETY: `retained` is owned; a null `out` is refused.
        unsafe { echode_snapshot_flicks(retained, 0, std::ptr::null_mut()) },
        ECHODE_ERR
    );
    assert_eq!(
        // SAFETY: null is an accepted document and the UUID has 16 readable bytes.
        unsafe { echode_document_track_peak(std::ptr::null_mut(), unaligned[1..].as_ptr()) },
        0.0
    );
    // SAFETY: `retained` is owned and freed once.
    unsafe { echode_snapshot_free(retained) };
}

#[test]
fn retained_audio_json_stays_owned_and_null_queries_clear_the_length() {
    let original = snapshot();
    // SAFETY: `original` is owned and not yet freed.
    let retained = unsafe { echode_snapshot_retain(original) };
    let mut length = 0usize;
    // SAFETY: `retained` is owned and `length` is writable.
    let pointer = unsafe { echode_snapshot_audio(retained, &mut length) };
    // SAFETY: `original` is owned and freed once; `retained` is a separate handle.
    unsafe { echode_snapshot_free(original) };
    let state: serde_json::Value =
        // SAFETY: `retained` still lends `length` bytes at `pointer`, though `original` was freed.
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(pointer, length) }).unwrap();
    assert_eq!(state["playing"], false);
    assert_eq!(state["tick"], 0);
    assert_eq!(state["devices"], serde_json::json!([]));
    // SAFETY: null is an accepted snapshot and `length` is writable.
    assert!(unsafe { echode_snapshot_audio(std::ptr::null(), &mut length) }.is_null());
    assert_eq!(length, 0);
    // SAFETY: `retained` is owned; a null length is refused.
    assert!(unsafe { echode_snapshot_audio(retained, std::ptr::null_mut()) }.is_null());
    // SAFETY: `retained` is owned and freed once.
    unsafe { echode_snapshot_free(retained) };
}

#[test]
fn visible_capacity_and_null_contract_leave_the_canary_intact() {
    let snapshot = snapshot();
    let empty = VisibleElement {
        role: u32::MAX,
        track_id: [0; 16],
        placement_id: [0; 16],
        note_id: [0; 16],
        has_placement: 0,
        has_note: 0,
        pad: [0; 2],
        x: 0.,
        y: 0.,
        w: 0.,
        h: 0.,
    };
    let mut elements = [empty; 2]; // One writable element followed by a canary, for this test only.
    let mut written = 0;
    let mut dropped = 0;
    // SAFETY: `snapshot` is owned until freed below; each `out` is null or `elements`, which holds
    // `capacity` elements, and the counts are separate locals.
    let mut query = |out, capacity, origin| unsafe {
        echode_snapshot_visible(
            snapshot,
            0,
            705_600,
            origin,
            1.,
            1.,
            64.,
            out,
            capacity,
            &mut written,
            &mut dropped,
        )
    };
    assert_eq!(query(std::ptr::null_mut(), 0, 0.), 0);
    assert_eq!(query(elements.as_mut_ptr(), 1, 0.), 0);
    assert_eq!(elements[0].role, ECHODE_ROLE_TRACK);
    assert_eq!(elements[1].role, u32::MAX);
    assert_eq!(query(elements.as_mut_ptr(), 1, f64::NAN), 0);
    assert_eq!(query(std::ptr::null_mut(), 1, 0.), -1);
    assert_eq!(written, 0);
    assert_eq!(dropped, 0);
    // SAFETY: `snapshot` is owned and freed once.
    unsafe { echode_snapshot_free(snapshot) };
}

#[test]
fn piano_buffers_and_navigation_keep_ids_alive_after_retain() {
    use crate::piano::{
        echode_snapshot_piano_neighbor, echode_snapshot_piano_note, echode_snapshot_piano_visible,
    };
    use echode_model::{NoteDraft, Tick};
    let track = Id::from_raw(1);
    let project = Project::new()
        .add_track_with_id(track, "keys", MixerIds::from_base(16))
        .unwrap()
        .insert_note_with_id(
            track,
            Id::from_raw(32),
            Id::from_raw(33),
            Id::from_raw(34),
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(705_600),
            },
        )
        .unwrap();
    let original = Box::into_raw(Box::new(Snapshot::from_project(Arc::new(project), None)));
    // SAFETY: `original` is owned and not yet freed.
    let retained = unsafe { echode_snapshot_retain(original) };
    // SAFETY: `original` is owned and freed once; `retained` is a separate handle.
    unsafe { echode_snapshot_free(original) };
    let track = track.raw().to_be_bytes();
    let canary = PianoNoteElement {
        pitch: u32::MAX,
        ..PianoNoteElement::default()
    };
    let mut out = [canary; 2]; // One output and a canary for this ABI test only.
    let (mut written, mut dropped) = (0, 0);
    // SAFETY: `retained` is owned until freed below, `track` has 16 readable bytes, each `output`
    // is null or `out`, which holds `capacity` elements, and the counts are separate locals.
    let mut query = |output, capacity, top| unsafe {
        echode_snapshot_piano_visible(
            retained,
            track.as_ptr(),
            0,
            705_600 * 4,
            top,
            2.,
            1. / 705_600.,
            20.,
            output,
            capacity,
            &mut written,
            &mut dropped,
        )
    };
    assert_eq!(query(std::ptr::null_mut(), 0, 60.), 0);
    assert_eq!(query(out.as_mut_ptr(), 1, 60.), 0);
    assert_eq!(out[0].pitch, 60);
    assert_eq!(out[1], canary);
    assert_eq!(query(std::ptr::null_mut(), 1, 60.), -1);
    assert_eq!((written, dropped), (0, 0));
    let mut resolved = canary;
    assert_eq!(
        // SAFETY: `retained` is owned, the UUIDs have 16 readable bytes each and `resolved` is
        // writable.
        unsafe {
            echode_snapshot_piano_note(
                retained,
                track.as_ptr(),
                out[0].placement_id.as_ptr(),
                out[0].note_id.as_ptr(),
                &mut resolved,
            )
        },
        0
    );
    assert_eq!(resolved.note_id, out[0].note_id);
    assert_eq!(
        // SAFETY: `retained` is owned, `track` has 16 readable bytes, the other UUIDs are null and
        // `resolved` is writable.
        unsafe {
            echode_snapshot_piano_neighbor(
                retained,
                track.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut resolved,
            )
        },
        0
    );
    assert_eq!(resolved.note_id, out[0].note_id);
    assert_eq!(
        // SAFETY: `retained` is owned, the UUIDs have 16 readable bytes each and `resolved` is
        // writable.
        unsafe {
            echode_snapshot_piano_neighbor(
                retained,
                track.as_ptr(),
                out[0].placement_id.as_ptr(),
                out[0].note_id.as_ptr(),
                0,
                &mut resolved,
            )
        },
        -1
    );
    // SAFETY: `retained` is owned and freed once.
    unsafe { echode_snapshot_free(retained) };
}
