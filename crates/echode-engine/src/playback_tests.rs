use super::*;
use std::sync::Arc;
use std::time::Instant;

#[path = "playback_preparation_tests.rs"]
mod background;
#[path = "playback_media_tests.rs"]
mod media_loading;
#[test]
fn every_option_rejects_a_negative_start_before_opening_helpers() {
    let project = Project::new();
    let cases = [
        PlaybackOptions::default().at(-1),
        PlaybackOptions::default().clock(Box::new(InternalClock { position: -1 })),
        PlaybackOptions::default()
            .plugins(std::path::Path::new("/unused"), &[])
            .at(-1),
    ];
    for options in cases {
        assert!(matches!(
            playback(&project, 48_000, 64, options),
            Err(PlaybackError::Time)
        ));
    }
}

#[test]
fn start_clock_and_plugins_combine_and_the_last_start_choice_wins() {
    let project = Project::new();
    let host = std::path::Path::new("/unused");
    let (control, _audio) = playback(
        &project,
        48_000,
        64,
        PlaybackOptions::default()
            .at(10)
            .plugins(host, &[])
            .clock(Box::new(InternalClock { position: 96 })),
    )
    .unwrap();
    assert_eq!(control.status().sample, 96);
    assert_eq!(control.host_bin, host);

    let (control, _audio) = playback(
        &project,
        48_000,
        64,
        PlaybackOptions::default()
            .clock(Box::new(InternalClock { position: 96 }))
            .at(10),
    )
    .unwrap();
    assert_eq!(control.status().sample, 10);
    assert!(control.host_bin.as_os_str().is_empty());
}

fn placed(track: u128, start: i64, value: f32, frames: usize) -> LoadedClip {
    LoadedClip {
        track,
        start_sample: start,
        samples: Arc::from(vec![value; frames]),
        right: None,
        fade_in: 0,
        fade_out: 0,
    }
}

#[test]
fn stereo_clips_loop_and_fader_replacements_run_under_rtsan() {
    let (project, track) = Project::new().add_track("stereo");
    let gain = project.tracks()[0].devices[0]
        .parameters
        .iter()
        .find(|parameter| parameter.key == echode_model::ParamKey::Gain)
        .unwrap()
        .id;
    let quiet = project.set_parameter(gain, 0.0).unwrap();
    let block = 64; // Scratch for this test, not a playback limit.
    let (mut control, mut audio) =
        playback(&project, 48_000, block, PlaybackOptions::default()).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| {
        let mut clip = placed(track.raw(), 0, 0.25, 2048);
        clip.right = Some(Arc::from(vec![-0.25; 2048]));
        Ok(vec![clip])
    }));
    control
        .command(PlaybackCommand::SetLoop {
            start: 337,
            end: 1360,
        })
        .unwrap();
    control.command(PlaybackCommand::Locate(337)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    for step in 0..32 {
        if step == 7 || step == 8 {
            control.replace(&quiet).unwrap();
        }
        audio.render(&mut left, &mut right);
        assert!(!control.status().failed);
        assert!(
            left.iter()
                .zip(&right)
                .all(|(left, right)| *left == -*right)
        );
        if step >= 11 {
            assert!(left.iter().all(|sample| *sample == 0.0));
        }
    }
}

#[test]
fn an_unheard_start_cancelled_after_a_stop_returned_keeps_the_start() {
    let (project, track) = Project::new().add_track("audio");
    let block = 64; // Scratch for this test, not a playback limit.
    let (mut control, mut audio) =
        playback(&project, 48_000, block, PlaybackOptions::default()).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| {
        Ok(vec![placed(track.raw(), 0, 0.25, 96_000)])
    }));
    control.replace(&project).unwrap();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    // Paused away from the start, then a play that is stopped before it is heard.
    control.command(PlaybackCommand::Locate(24_000)).unwrap();
    audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    control.command(PlaybackCommand::Play).unwrap();
    control.command(PlaybackCommand::Pause).unwrap();
    control.command(PlaybackCommand::Rewind(0)).unwrap();
    audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    audio.cancel_unheard_start(24_000);
    assert_eq!(control.status().sample, 0, "the cancel undid the return");
    assert!(!control.status().playing);
    // A start cancelled while still playing goes back to where it began.
    control.command(PlaybackCommand::Play).unwrap();
    audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    assert_eq!(control.status().sample, block as i64);
    audio.cancel_unheard_start(0);
    assert_eq!(control.status().sample, 0);
}

#[test]
fn state_polling_does_not_change_the_current_plan() {
    let (mut control, mut audio) =
        playback(&Project::new(), 48000, 64, PlaybackOptions::default()).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = [0.0; 64];
    let mut right = left;
    for _ in 0..8 {
        control.service();
        audio.render(&mut left, &mut right);
    }
    assert_eq!(control.status().sample, 512);
    assert_eq!(control.status().graph_unavailable_blocks, 0);
}

#[test]
fn each_callback_publishes_each_track_meter_once() {
    let (project, track) = Project::new().add_track("one meter store");
    let block = 128; // Scratch for this test, not a playback limit.
    let (mut control, mut audio) =
        playback(&project, 48_000, block, PlaybackOptions::default()).unwrap();
    let clip = placed(track.raw(), 0, 0.5, 4_096);
    control.set_clip_loader(Arc::new(move |_, _| Ok(vec![clip.clone()])));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let meter = control.meters().pop().unwrap();
    let mut left = vec![0.0f32; block];
    let mut right = vec![0.0f32; block];
    let deadline = Instant::now() + std::time::Duration::from_secs(1);
    for index in 0..4 {
        audio.render_live_gated_until(&mut left, &mut right, deadline, true);
        assert_eq!(
            meter.publications(),
            index + 1,
            "callback {index} stored the peak more than once"
        );
    }
    let held = meter.peak();
    assert!(held > 0.2, "{held}");
    audio.render_live_gated_until(&mut left, &mut right, deadline, true);
    assert_eq!(meter.publications(), 5);
    assert!(
        (meter.peak() - held).abs() < 1.0e-3,
        "sustained peak moved from {held} to {}",
        meter.peak()
    );
}

fn callback_block(mut clips: Vec<LoadedClip>) -> Vec<f32> {
    let project = Project::new().add_track("file").0;
    let track = project.tracks()[0].id.raw();
    for clip in &mut clips {
        clip.track = track;
    }
    let (mut control, mut audio) =
        playback(&project, 48_000, 256, PlaybackOptions::default()).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; 256];
    let mut right = vec![0.0; 256];
    audio.render(&mut left, &mut right);
    assert!(!control.status().failed);
    left
}

#[test]
fn audit_aud12_future_clips_leave_the_callback_unchanged() {
    let body = placed(0, 0, 0.25, 256);
    let alone = callback_block(vec![body.clone()]);
    let mut crowded = vec![body];
    for index in 0..8_000 {
        crowded.push(placed(0, 1_000_000 + i64::from(index) * 300, 0.9, 256));
    }
    assert_eq!(callback_block(crowded), alone);
    assert!(alone.iter().any(|sample| *sample != 0.0));
    assert!(
        callback_block(vec![placed(0, -256, 0.9, 256)])
            .iter()
            .all(|sample| *sample == 0.0)
    );
    assert!(
        callback_block(vec![placed(0, 256, 0.9, 256)])
            .iter()
            .all(|sample| *sample == 0.0)
    );
}

#[test]
#[ignore = "timing measurement; `make spike-engine` runs it in a release build"]
fn future_clips_do_not_extend_the_callback() {
    const CALLBACKS: u32 = 2_000;
    let time = |future: usize| {
        let mut clips = vec![placed(0, 0, 0.25, 256)];
        for index in 0..future {
            clips.push(placed(
                0,
                1_000_000_000 + i64::try_from(index).unwrap_or(i64::MAX) * 300,
                0.9,
                64,
            ));
        }
        let project = Project::new().add_track("file").0;
        let track = project.tracks()[0].id.raw();
        for clip in &mut clips {
            clip.track = track;
        }
        let (mut control, mut audio) =
            playback(&project, 48_000, 256, PlaybackOptions::default()).unwrap();
        control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
        control.replace(&project).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut left = vec![0.0; 256];
        let mut right = vec![0.0; 256];
        audio.render(&mut left, &mut right);
        let started = Instant::now();
        for _ in 0..CALLBACKS {
            audio.render(&mut left, &mut right);
        }
        started.elapsed()
    };
    let near = time(0);
    let far = time(100_000);
    println!("{CALLBACKS} callbacks, 1 sounding clip: {near:?}; plus 100000 future clips: {far:?}");
    let ratio = far.as_secs_f64() / near.as_secs_f64().max(1.0e-6);
    assert!(
        ratio < 3.0,
        "callback time grew {ratio:.2}× with clips that do not sound"
    );
}

#[test]
fn ten_thousand_product_replacements_and_transport_commands_run_under_rtsan() {
    let project = Project::new().add_track("silent").0;
    let (mut control, mut audio) =
        playback(&project, 48000, 64, PlaybackOptions::default()).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    for index in 0..10000 {
        control.replace(&project).unwrap();
        control
            .command(match index % 4 {
                0 => PlaybackCommand::Play,
                1 => PlaybackCommand::Pause,
                2 => PlaybackCommand::Locate(index),
                _ => PlaybackCommand::Stop,
            })
            .unwrap();
        audio.render(&mut left, &mut right);
        assert!(!control.status().failed);
    }
}

fn project_with_notes(count: usize) -> Project {
    use echode_model::{Id, NoteDraft, Tick};
    let (project, track) = Project::new().add_track("piano");
    if count == 0 {
        return project;
    }
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(1),
            },
        )
        .unwrap();
    let mut content = project.contents()[0].as_ref().clone();
    let note = content.notes[0].clone();
    // Fixture construction is outside the measured playback preparation.
    // Avoid rebuilding/cloning the growing model once per note. Assemble
    // once through the public invariant checker with the same note geometry.
    content.notes = (0..count)
        .map(|index| echode_model::Note {
            id: Id::generate(),
            start: Tick::new(index as i64 * 48_000),
            ..note.clone()
        })
        .collect();
    let mut track = project.tracks()[0].as_ref().clone();
    let end = (count as i64 - 1) * 48_000 + 1;
    let bar = project.meter().bar_ticks().unwrap().get();
    track.clips[0].length = Tick::new((end + bar - 1).div_euclid(bar) * bar);
    Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        vec![content],
        vec![track],
    )
    .unwrap()
}

#[test]
fn audit_rea01_a_loop_free_plan_does_not_scale_events_with_the_block() {
    for notes in [100usize, 1_000] {
        let project = project_with_notes(notes);
        let mut counts = Vec::new();
        for frames in [1usize, 64, 128, 512] {
            let (control, _audio) =
                playback(&project, 48_000, frames, PlaybackOptions::default()).unwrap();
            counts.push(control.reserved_events());
        }
        assert!(
            counts.iter().all(|count| *count == counts[0]),
            "{notes} notes scaled with the block: {counts:?}"
        );
        assert!(
            counts[0] < notes.saturating_mul(8),
            "{notes} reserved {}",
            counts[0]
        );
    }
    use echode_model::{NoteDraft, Tick};
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(4).unwrap(),
            },
        )
        .unwrap()
        .0;
    let (mut control, _audio) =
        playback(&project, 48_000, 512, PlaybackOptions::default()).unwrap();
    let open = control.reserved_events();
    control
        .command(PlaybackCommand::SetLoop { start: 0, end: 1 })
        .unwrap();
    assert!(control.reserved_events() > open);
    control.command(PlaybackCommand::ClearLoop).unwrap();
    assert_eq!(control.reserved_events(), open);
}

#[test]
#[ignore = "timing measurement; `make spike-engine` runs it in a release build"]
fn many_notes_reserve_events_without_the_block_length() {
    for notes in [10_000usize, 100_000] {
        let project = project_with_notes(notes);
        let small = playback(&project, 48_000, 1, PlaybackOptions::default())
            .unwrap()
            .0
            .reserved_events();
        let large = playback(&project, 48_000, 512, PlaybackOptions::default())
            .unwrap()
            .0
            .reserved_events();
        println!("events, {notes} notes, 1/512 frames: {small}/{large}");
        assert_eq!(small, large);
        assert!(large < notes.saturating_mul(8));
    }
}

#[test]
fn audit_pro06_a_hundred_sample_loop_keeps_playing_in_a_large_block() {
    use echode_model::{NoteDraft, Tick};
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(4).unwrap(),
            },
        )
        .unwrap()
        .0;
    for frames in [64usize, 512] {
        let (mut control, mut audio) =
            playback(&project, 48_000, frames, PlaybackOptions::default()).unwrap();
        control.set_clip_loader(Arc::new(move |_, _| {
            Ok(vec![placed(track.raw(), 0, 0.25, 100)])
        }));
        control
            .command(PlaybackCommand::SetLoop { start: 0, end: 100 })
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut left = vec![0.0; frames];
        let mut right = vec![0.0; frames];
        audio.render(&mut left, &mut right);
        let peak = left
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak > 0.0, "{frames} frames were silent");
        let status = control.status();
        assert!(!status.failed, "{frames} frames failed");
        assert!(status.playing, "{frames} frames stopped");
    }
}

#[test]
fn a_short_loop_wraps_and_the_metronome_clicks_on_the_downbeat() {
    use echode_model::{TICKS_PER_QUARTER, Tick};
    let project = Project::new();
    let (mut control, mut audio) =
        playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    let quarter =
        crate::tick_to_sample(Tick::new(TICKS_PER_QUARTER), project.tempo(), 48_000).unwrap();
    control
        .command(PlaybackCommand::SetLoop {
            start: 0,
            end: quarter,
        })
        .unwrap();
    control
        .command(PlaybackCommand::SetMetronome(true))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    audio.render(&mut left, &mut right);
    assert_ne!(left[0], 0.0);
    assert!(!control.status().failed);
    let mut rendered = 64i64;
    while rendered <= quarter {
        audio.render(&mut left, &mut right);
        rendered += 64;
        assert!(!control.status().failed);
    }
    assert!(control.status().sample < quarter);
}

#[test]
fn audit_aud04_replacing_a_host_graph_and_sorting_events_does_not_allocate() {
    use crate::realtime::ForbidAlloc;
    use echode_realtime::{Event, sort_events};
    let mut events = Vec::with_capacity(1002);
    for index in 0..1000u32 {
        events.push(Event::note_on(1000 - index, 1, index, 60, 0.5));
    }
    events.insert(0, Event::note_on(40, 1, 7, 60, 0.2));
    events.insert(1, Event::note_on(40, 1, 7, 60, 0.8));
    events.insert(2, Event::note_off(40, 1, 7, 60));
    let mut order = vec![0usize; events.len()];
    let mut scratch = vec![Event::note_off(0, 0, 0, 0); events.len()];
    {
        let _guard = ForbidAlloc::enter();
        assert!(sort_events(&mut events, &mut order, &mut scratch));
    }
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].time() <= pair[1].time())
    );
    let at_forty: Vec<_> = events
        .iter()
        .filter(|event| event.time() == 40 && event.note_id() == Some(7))
        .map(|event| (event.is_note_off(), event.velocity()))
        .collect();
    assert_eq!(at_forty, vec![(true, 0.0), (false, 0.2), (false, 0.8)]);

    let project = Project::new().add_track("a").0;
    let (mut control, mut audio) = playback(
        &project,
        48_000,
        64,
        PlaybackOptions::default().plugins(std::path::Path::new("/unused"), &[]),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    audio.render(&mut left, &mut right);
    let grown = project.add_track("b").0;
    control.replace(&grown).unwrap();
    {
        let _guard = ForbidAlloc::enter();
        audio.render(&mut left, &mut right);
    }
    control.replace(&project).unwrap();
    {
        let _guard = ForbidAlloc::enter();
        audio.render(&mut left, &mut right);
    }
    assert!(!control.status().failed);
}
