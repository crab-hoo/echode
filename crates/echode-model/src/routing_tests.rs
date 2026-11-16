use std::sync::Arc;

use crate::{
    GAIN_MAX, Id, InvariantError, ModelError, Project, ROUTE_GAIN_DEFAULT, RouteTap, SendRoute,
};

fn round_trip(before: &Project, after: &Project) {
    after.check_invariants().unwrap();
    let diff = before.diff(after);
    assert_eq!(before.apply_changeset(&diff).as_ref(), Ok(after));
    assert_eq!(after.apply_changeset(&diff.invert()).as_ref(), Ok(before));
}

#[test]
fn sends_and_sidechains_edit_query_and_round_trip_through_changesets() {
    let (project, kick) = Project::new().add_track("Kick");
    let (project, bass) = project.add_track("Bass");
    let (project, reverb) = project.add_track("Reverb");
    let comp = Id::generate();
    let project = project
        .add_clap_device(bass, comp, "org.example.comp")
        .unwrap();

    let (with_send, send_id) = project.add_send(bass, reverb, ROUTE_GAIN_DEFAULT).unwrap();
    round_trip(&project, &with_send);
    let (owner, send) = with_send.send(send_id).unwrap();
    assert_eq!(owner, bass);
    assert_eq!(send.target, reverb);
    assert_eq!(send.tap, RouteTap::PostFxPreFader);
    assert_eq!(send.gain.to_bits(), 1.0f32.to_bits());
    assert!(Arc::ptr_eq(&project.tracks()[0], &with_send.tracks()[0]));

    let trimmed = with_send.set_send_gain(send_id, -0.0).unwrap();
    round_trip(&with_send, &trimmed);
    assert_eq!(
        trimmed.send(send_id).unwrap().1.gain.to_bits(),
        0.0f32.to_bits()
    );
    assert!(
        trimmed
            .diff(&trimmed.set_send_gain(send_id, 0.0).unwrap())
            .is_empty()
    );

    let (with_sc, sc_id) = trimmed.add_sidechain(kick, comp, 0.75).unwrap();
    round_trip(&trimmed, &with_sc);
    let (sc_track, sc_dev, sc) = with_sc.sidechain(sc_id).unwrap();
    assert_eq!((sc_track, sc_dev, sc.source), (bass, comp, kick));
    assert_eq!(sc.tap, RouteTap::PostFxPreFader);
    assert_eq!(sc.gain, 0.75);

    let loud_sc = with_sc.set_sidechain_gain(sc_id, GAIN_MAX).unwrap();
    round_trip(&with_sc, &loud_sc);
    assert_eq!(loud_sc.sidechain(sc_id).unwrap().2.gain, GAIN_MAX);

    let no_sc = loud_sc.remove_sidechain(sc_id).unwrap();
    round_trip(&loud_sc, &no_sc);
    assert!(no_sc.sidechain(sc_id).is_none());

    let no_send = no_sc.remove_send(send_id).unwrap();
    round_trip(&no_sc, &no_send);
    assert_eq!(no_send, project);
}

#[test]
fn routes_reject_invalid_gains_missing_endpoints_mixers_and_duplicates() {
    let (project, a) = Project::new().add_track("A");
    let (project, b) = project.add_track("B");
    let fx_one = Id::generate();
    let fx_two = Id::generate();
    let project = project
        .add_clap_device(b, fx_one, "org.example.fx")
        .unwrap()
        .add_clap_device(b, fx_two, "org.example.fx")
        .unwrap();
    let mixer = project.tracks()[1].devices.last().unwrap().id;

    for bad_gain in [
        -0.01,
        GAIN_MAX + 0.01,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        assert_eq!(
            project.add_send(a, b, bad_gain),
            Err(ModelError::InvalidGain)
        );
        assert_eq!(
            project.add_sidechain(a, fx_one, bad_gain),
            Err(ModelError::InvalidGain)
        );
    }
    assert_eq!(
        project.add_send(Id::generate(), b, 1.0),
        Err(ModelError::MissingTrack)
    );
    assert_eq!(
        project.add_send(a, Id::generate(), 1.0),
        Err(ModelError::MissingTrack)
    );
    assert_eq!(
        project.add_sidechain(Id::generate(), fx_one, 1.0),
        Err(ModelError::MissingTrack)
    );
    assert_eq!(
        project.add_sidechain(a, mixer, 1.0),
        Err(ModelError::InvalidDevice)
    );
    assert_eq!(
        project.add_sidechain(a, Id::generate(), 1.0),
        Err(ModelError::InvalidDevice)
    );
    assert_eq!(
        project.set_send_gain(Id::generate(), 1.0),
        Err(ModelError::MissingRoute)
    );
    assert_eq!(
        project.remove_sidechain(Id::generate()),
        Err(ModelError::MissingRoute)
    );

    let (with_send, _) = project.add_send(a, b, 1.0).unwrap();
    assert_eq!(
        with_send.add_send(a, b, 0.5),
        Err(ModelError::DuplicateRoute)
    );

    // One source cannot feed the same CLAP device twice, but two devices on the
    // same track may share a sidechain source.
    let (with_sc, _) = with_send.add_sidechain(a, fx_one, 1.0).unwrap();
    assert_eq!(
        with_sc.add_sidechain(a, fx_one, 0.5),
        Err(ModelError::DuplicateRoute)
    );
    let (both_devices, _) = with_sc.add_sidechain(a, fx_two, 0.5).unwrap();
    both_devices.check_invariants().unwrap();
}

#[test]
fn routing_graph_rejects_cycles_across_sends_sidechains_and_folders() {
    let (project, folder) = Project::new().add_track("Drums");
    let (project, kick) = project.add_track("Kick");
    let (project, bus) = project.add_track("Bus");
    let folder_fx = Id::generate();
    let kick_fx = Id::generate();
    let bus_fx = Id::generate();
    let project = project
        .set_parent(kick, Some(folder))
        .unwrap()
        .add_clap_device(folder, folder_fx, "org.example.comp")
        .unwrap()
        .add_clap_device(kick, kick_fx, "org.example.comp")
        .unwrap()
        .add_clap_device(bus, bus_fx, "org.example.comp")
        .unwrap();

    // Self-loops are cycles.
    assert_eq!(project.add_send(kick, kick, 1.0), Err(ModelError::Cycle));
    assert_eq!(
        project.add_sidechain(kick, kick_fx, 1.0),
        Err(ModelError::Cycle)
    );

    // Folder receives `kick -> folder`, so `folder -> kick` loops while parallel
    // `kick -> folder` send/sidechain stays acyclic.
    assert_eq!(project.add_send(folder, kick, 1.0), Err(ModelError::Cycle));
    assert_eq!(
        project.add_sidechain(folder, kick_fx, 1.0),
        Err(ModelError::Cycle)
    );
    let (parallel, _) = project.add_send(kick, folder, 0.5).unwrap();
    let (parallel, _) = parallel.add_sidechain(kick, folder_fx, 1.0).unwrap();
    parallel.check_invariants().unwrap();

    // Multi-hop `kick -> folder -> bus -> kick` loop via send + sidechain, and
    // reparenting `bus` under `kick` when `kick -> bus` already exists.
    let (chained, _) = project.add_send(folder, bus, 1.0).unwrap();
    assert_eq!(
        chained.add_sidechain(bus, kick_fx, 1.0),
        Err(ModelError::Cycle)
    );
    assert_eq!(chained.set_parent(bus, Some(kick)), Err(ModelError::Cycle));
}

#[test]
fn device_removal_copy_and_track_duplication_maintain_route_invariants() {
    let (project, kick) = Project::new().add_track("Kick");
    let (project, folder) = project.add_track("Synth Group");
    let (project, lead) = project.add_track("Lead");
    let (project, pad) = project.add_track("Pad");
    let (project, reverb) = project.add_track("Reverb");
    let lead_fx = Id::generate();
    let pad_fx = Id::generate();
    let project = project
        .set_parent(lead, Some(folder))
        .unwrap()
        .set_parent(pad, Some(folder))
        .unwrap()
        .add_clap_device(lead, lead_fx, "org.example.comp")
        .unwrap()
        .add_clap_device(pad, pad_fx, "org.example.comp")
        .unwrap();
    // External sidechain (`kick -> lead_fx`), internal sidechain (`lead -> pad_fx`),
    // internal send (`lead -> pad`), and external send (`pad -> reverb`).
    let (project, _) = project.add_sidechain(kick, lead_fx, 1.0).unwrap();
    let (project, _) = project.add_sidechain(lead, pad_fx, 0.8).unwrap();
    let (project, _) = project.add_send(lead, pad, 0.5).unwrap();
    let (project, _) = project.add_send(pad, reverb, 0.25).unwrap();

    // Removing a CLAP device drops its sidechains and round-trips through Undo.
    let removed = project.remove_device(lead_fx).unwrap();
    round_trip(&project, &removed);

    // Copying `lead_fx` onto `reverb` keeps the `kick` sidechain under a fresh id,
    // while copying it onto `kick` itself is rejected as a self-cycle.
    let copied_fx = Id::generate();
    let copied = project
        .copy_device(lead_fx, reverb, copied_fx, None)
        .unwrap();
    round_trip(&project, &copied);
    assert_eq!(
        project.copy_device(lead_fx, kick, Id::generate(), None),
        Err(ModelError::Cycle)
    );

    // Duplicating `folder` remaps internal `lead -> pad` routes inside the copy
    // while keeping external `kick` sidechain and `reverb` send targets intact.
    let duplicated = project.duplicate_track(folder, Id::generate()).unwrap();
    round_trip(&project, &duplicated);
    assert_eq!(duplicated.tracks().len(), 8);
    let copied_lead = &duplicated.tracks()[5];
    let copied_pad = &duplicated.tracks()[6];
    assert_eq!(copied_lead.sends[0].target, copied_pad.id);
    assert_eq!(copied_pad.sends[0].target, reverb);
    assert_eq!(copied_lead.devices[0].sidechains[0].source, kick);
    assert_eq!(copied_pad.devices[0].sidechains[0].source, copied_lead.id);
}

#[test]
fn unchecked_cycle_is_caught_by_check_invariants_and_deep_chains_do_not_overflow() {
    let mut project = Project::new();
    let mut ids = Vec::new();
    for index in 0..512 {
        let (next, track) = project.add_track(format!("t{index}"));
        project = next;
        ids.push(track);
    }
    for pair in ids.windows(2) {
        project = project.add_send(pair[0], pair[1], 1.0).unwrap().0;
    }
    project.check_invariants().unwrap();

    // Corrupting the tail track with a back-edge send must surface RouteCycle.
    let mut tracks: Vec<_> = project.tracks().to_vec();
    let mut last = (*tracks.pop().unwrap()).clone();
    last.sends
        .push(SendRoute::new(Id::generate(), ids[0], 1.0).unwrap());
    tracks.push(Arc::new(last));
    let broken = project.with_parts(project.contents().to_vec(), tracks);
    assert!(matches!(
        broken.check_invariants(),
        Err(InvariantError::RouteCycle { .. })
    ));
}
