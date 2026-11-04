use super::*;

fn render_block(audio: &mut PlaybackAudio) {
    // Scratch for command routing only, not a playback or track limit.
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    audio.render(&mut left, &mut right);
}

#[test]
fn public_transport_commands_keep_their_order_and_receipts_under_rtsan() {
    let (mut control, mut audio) =
        playback(&Project::new(), 48_000, 64, PlaybackOptions::default()).unwrap();
    let cases = [
        (PlaybackCommand::Locate(137), false, 137),
        (PlaybackCommand::Play, true, 201),
        (PlaybackCommand::Pause, false, 201),
        (PlaybackCommand::Play, true, 265),
        (PlaybackCommand::Stop, false, 265),
        (PlaybackCommand::Rewind(211), false, 211),
        (PlaybackCommand::SetMetronome(true), false, 211),
        (PlaybackCommand::SetMetronome(false), false, 211),
        (
            PlaybackCommand::ClearFaderPreview { after_plan: 0 },
            false,
            211,
        ),
    ];
    for (index, (command, playing, sample)) in cases.into_iter().enumerate() {
        control.command(command).unwrap();
        let revision = index as u64 + 1;
        assert_eq!(control.command_revision(), revision);
        assert_eq!(control.status().applied_command, revision - 1);
        render_block(&mut audio);
        let status = control.status();
        assert!(!status.failed, "command {command:?} failed");
        assert_eq!(status.applied_command, revision);
        assert_eq!(status.playing, playing);
        assert_eq!(status.sample, sample);
        if let PlaybackCommand::SetMetronome(enabled) = command {
            assert_eq!(audio.metronome, enabled);
        }
    }
    // A callback drains commands in submission order, not enum/variant order.
    control.command(PlaybackCommand::Play).unwrap();
    control.command(PlaybackCommand::Rewind(701)).unwrap();
    control.command(PlaybackCommand::Locate(919)).unwrap();
    render_block(&mut audio);
    assert_eq!(control.status().sample, 919);
    assert!(!control.status().playing);
    assert_eq!(control.status().applied_command, control.command_revision());
}

#[test]
fn loop_commands_prepare_plans_without_entering_the_command_ring_under_rtsan() {
    let (mut control, mut audio) =
        playback(&Project::new(), 48_000, 64, PlaybackOptions::default()).unwrap();
    control.command(PlaybackCommand::Locate(11)).unwrap();
    let cases = [
        (
            PlaybackCommand::SetLoop { start: 17, end: 99 },
            Some((17, 99)),
        ),
        (PlaybackCommand::ClearLoop, None),
        (PlaybackCommand::SetLoop { start: 8, end: 8 }, None),
        (PlaybackCommand::SetLoop { start: 9, end: 8 }, None),
    ];
    for (command, bounds) in cases {
        let command_slots = control.commands.slots();
        let requested = control.status.requested_plan.load(Ordering::Acquire);
        control.command(command).unwrap();
        assert_eq!(control.loop_bounds, bounds);
        assert_eq!(control.command_revision(), 1);
        assert_eq!(control.commands.slots(), command_slots);
        assert_ne!(
            control.status.requested_plan.load(Ordering::Acquire),
            requested
        );
        render_block(&mut audio);
        assert_eq!((audio.loop_start, audio.loop_end), bounds.unzip());
        assert_eq!(control.status().sample, 11);
        assert_eq!(control.status().applied_command, 1);
        assert!(!control.status().failed);
        control.service();
    }
}

#[test]
fn single_fader_commands_validate_and_queue_owned_groups_under_rtsan() {
    let (project, track) = Project::new().add_track("preview");
    let (mut control, mut audio) =
        playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    for (id, gain, pan) in [
        (track.raw(), f32::NAN, 0.0),
        (track.raw(), -0.1, 0.0),
        (track.raw(), 4.1, 0.0),
        (track.raw(), 1.0, f32::INFINITY),
        (track.raw(), 1.0, -1.1),
        (track.raw(), 1.0, 1.1),
        (echode_model::Id::generate().raw(), 1.0, 0.0),
    ] {
        assert!(matches!(
            control.command(PlaybackCommand::PreviewFader {
                track: id,
                gain,
                pan
            }),
            Err(PlaybackError::InvalidFader)
        ));
        assert_eq!(control.command_revision(), 0);
        assert!(audio.commands.peek().is_err());
    }
    control
        .command(PlaybackCommand::PreviewFader {
            track: track.raw(),
            gain: 0.25,
            pan: -1.0,
        })
        .unwrap();
    assert!(
        matches!(audio.commands.peek(), Ok((1, QueuedCommand::Faders(values)))
        if values.as_ref() == [(track.raw(), 0.25, -1.0)])
    );
    render_block(&mut audio);
    assert_eq!(
        audio.fader_preview.as_deref(),
        Some([(track.raw(), 0.25, -1.0)].as_slice())
    );
    assert_eq!(control.status().applied_command, 1);
    control.clear_fader_preview(false).unwrap();
    render_block(&mut audio);
    assert!(audio.fader_preview.is_none());
    assert_eq!(control.status().applied_command, 2);
    assert_eq!(control.preview_returns.slots(), 1);
    control.service();
    assert_eq!(control.preview_returns.slots(), 0);
}

#[test]
fn a_full_command_ring_rejects_without_advancing_the_receipt_under_rtsan() {
    let (project, track) = Project::new().add_track("queue");
    let (mut control, mut audio) =
        playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    let depth = control.commands.slots();
    for sample in 0..depth {
        control
            .command(PlaybackCommand::Locate(sample as i64))
            .unwrap();
    }
    let revision = control.command_revision();
    for command in [
        PlaybackCommand::Play,
        PlaybackCommand::PreviewFader {
            track: track.raw(),
            gain: 0.5,
            pan: 0.0,
        },
    ] {
        assert!(matches!(
            control.command(command),
            Err(PlaybackError::BufferTooSmall)
        ));
        assert_eq!(control.command_revision(), revision);
    }
    render_block(&mut audio);
    assert_eq!(control.status().sample, depth as i64 - 1);
    assert_eq!(control.status().applied_command, revision);
    assert!(!control.status().playing);
    control.command(PlaybackCommand::Play).unwrap();
    render_block(&mut audio);
    assert!(control.status().playing);
    assert_eq!(control.status().applied_command, revision + 1);
}

#[test]
fn preview_return_backpressure_keeps_commands_until_control_reclaims_under_rtsan() {
    let (project, track) = Project::new().add_track("returns");
    let (mut control, mut audio) =
        playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    let depth = audio.preview_returns.slots();
    assert_eq!(control.commands.slots(), depth);
    for _ in 0..depth {
        control.preview_fader(track.raw(), 0.5, 0.0).unwrap();
    }
    render_block(&mut audio);
    assert_eq!(control.preview_returns.slots(), depth - 1);
    control.clear_fader_preview(false).unwrap();
    render_block(&mut audio);
    assert_eq!(control.preview_returns.slots(), depth);
    let applied = control.status().applied_command;
    control.command(PlaybackCommand::Play).unwrap();
    render_block(&mut audio);
    assert_eq!(control.status().applied_command, applied);
    assert!(!control.status().playing);
    control.service();
    render_block(&mut audio);
    assert!(control.status().playing);
    assert_eq!(control.status().applied_command, applied + 1);
    assert!(!control.status().failed);
}

#[test]
fn full_plan_returns_leave_updates_queued_but_apply_previews_under_rtsan() {
    let (project, track) = Project::new().add_track("plans");
    let (mut control, mut audio) =
        playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    let depth = audio.returns.slots();
    let mut updates = Vec::new();
    for _ in 0..=depth {
        control.replace(&project).unwrap();
        // Hold real prepared updates off audio so control's automatic service
        // cannot reclaim returns between the callbacks in this fixture.
        updates.push(audio.plans.pop().unwrap());
    }
    let mut installed = 0;
    for (index, update) in updates.into_iter().enumerate() {
        let revision = update.revision;
        assert!(control.plans.push(update).is_ok());
        render_block(&mut audio);
        if index < depth {
            installed = revision;
        }
        assert_eq!(
            control.status.applied_plan.load(Ordering::Acquire),
            installed
        );
    }
    assert_eq!(audio.returns.slots(), 0);
    assert!(audio.plans.peek().is_ok());
    control.preview_fader(track.raw(), 0.25, 0.0).unwrap();
    render_block(&mut audio);
    assert_eq!(control.status().applied_command, control.command_revision());
    assert_eq!(
        audio.fader_preview.as_deref(),
        Some([(track.raw(), 0.25, 0.0)].as_slice())
    );
    assert_eq!(
        control.status.applied_plan.load(Ordering::Acquire),
        installed
    );
    control.service();
    render_block(&mut audio);
    assert!(!control.status().pending_plan);
    assert!(audio.plans.peek().is_err());
    assert!(!control.status().failed);
}
