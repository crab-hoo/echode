use super::*;
use crate::CMD_SHUTDOWN;
use echode_realtime::{Event, NoteEvent, Target};
use std::thread;
use std::time::Instant;

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "echode-{name}-{}-{}",
        std::process::id(),
        SHM_SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn replacement_control_wait_observes_the_stable_exit_notice() {
    let root = std::sync::Arc::new(MappedRegion::create_shared(SessionLayout::stereo()).unwrap());
    let mut next = MappedRegion::create_shared(SessionLayout::stereo()).unwrap();
    next.set_wake_target(std::sync::Arc::clone(&root));
    root.mark_aborted();
    // This is a missing-notification guard, not a performance threshold.
    assert!(matches!(
        next.activate_mapping(Duration::from_millis(100)),
        Err(RegionError::Dead)
    ));
}

#[test]
fn empty_and_zero_frame_layouts_are_rejected() {
    assert!(
        SessionLayout {
            max_frames: 0,
            input_channels: 2,
            output_channels: 2,
            input_events: 0,
            output_events: 0,
        }
        .validate()
        .is_err()
    );
    assert!(
        SessionLayout {
            max_frames: 16,
            input_channels: 0,
            output_channels: 0,
            input_events: 0,
            output_events: 0,
        }
        .validate()
        .is_err()
    );
    assert!(
        SessionLayout {
            max_frames: 16,
            input_channels: 0,
            output_channels: 0,
            input_events: 1,
            output_events: 0,
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn event_regions_do_not_overlap() {
    let path = temp_path("layout");
    let _ = std::fs::remove_file(&path);
    let layout = SessionLayout {
        max_frames: 4,
        input_channels: 1,
        output_channels: 1,
        input_events: 1,
        output_events: 1,
    };
    let region = MappedRegion::create_with(&path, layout).expect("region");
    let input_audio = region.slot_offset(1) + SLOT_HEAD;
    let output_audio = input_audio + 4 * std::mem::size_of::<f32>();
    assert!(input_audio + 4 * std::mem::size_of::<f32>() <= output_audio);
    let input_event = region.event_offset(1, false, 0);
    let output_event = region.event_offset(1, true, 0);
    assert!(input_event + RECORD_BYTES <= output_event);
    assert!(region.slot_offset(1) + region.stride <= region.slot_offset(2));
    drop(region);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_full_pair_of_slots_is_not_overwritten() {
    let path = temp_path("busy");
    let _ = std::fs::remove_file(&path);
    let region = MappedRegion::create(&path).expect("region");
    let input: [&[f32]; 2] = [&[0.0], &[0.0]];
    // Two blocks no helper completes fill both slots.
    for _ in 0..2 {
        region.submit_block(1, &input, &[]).expect("submit");
    }
    let mut left = [0.0f32];
    let mut right = [0.0f32];
    let mut output: [&mut [f32]; 2] = [&mut left, &mut right];
    let started = Instant::now();
    let error = region
        .request(1, &input, &mut output, CMD_PROCESS, Duration::from_secs(30))
        .expect_err("both slots are busy");
    assert!(matches!(error, RegionError::Busy));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(region.issued(), 2);
    assert_eq!(region.published(), 2);
    drop(region);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn zero_input_channels_round_trip_events() {
    let path = temp_path("synth");
    let _ = std::fs::remove_file(&path);
    let layout = SessionLayout {
        max_frames: 32,
        input_channels: 0,
        output_channels: 4,
        input_events: 4,
        output_events: 4,
    };
    let parent = MappedRegion::create_with(&path, layout).expect("region");
    let child_path = path.clone();
    let helper = thread::spawn(move || {
        let region = MappedRegion::open(&child_path).expect("open");
        let mut seen = 0u64;
        loop {
            let published = region.published();
            if published > seen {
                let generation = seen + 1;
                if region.block_command(generation) == CMD_SHUTDOWN {
                    region.finish(generation);
                    return;
                }
                region.copy_inputs_to_outputs(generation).expect("copy");
                region.echo_events(generation);
                region.finish(generation);
                seen = generation;
                continue;
            }
            let alarm = region.alarm();
            region.wait_alarm(alarm).expect("alarm");
        }
    });
    let mut first = vec![1.0f32; 8];
    let mut second = vec![1.0f32; 8];
    let mut third = vec![1.0f32; 8];
    let mut fourth = vec![1.0f32; 8];
    let mut planes: [&mut [f32]; 4] = [&mut first, &mut second, &mut third, &mut fourth];
    let note = Event::NoteOn(NoteEvent {
        time: 2,
        track: 1,
        target: Target::note(4, 60),
        velocity: 0.8,
    });
    let mut decoded = [note, note, note, note];
    let count = parent
        .request_events(
            8,
            &[],
            &mut planes,
            &[note],
            &mut decoded,
            CMD_PROCESS,
            Duration::from_secs(2),
        )
        .expect("process");
    assert_eq!(count, 1);
    assert_eq!(decoded[0], note);
    assert!(
        [first, second, third, fourth]
            .iter()
            .all(|plane| plane.iter().all(|sample| *sample == 0.0))
    );
    let mut tail_a = vec![0.0f32; 1];
    let mut tail_b = vec![0.0f32; 1];
    let mut tail_c = vec![0.0f32; 1];
    let mut tail_d = vec![0.0f32; 1];
    let mut tail_planes: [&mut [f32]; 4] = [&mut tail_a, &mut tail_b, &mut tail_c, &mut tail_d];
    let mut ignored = [];
    parent
        .request_events(
            1,
            &[],
            &mut tail_planes,
            &[],
            &mut ignored,
            CMD_SHUTDOWN,
            Duration::from_secs(2),
        )
        .expect("shutdown");
    helper.join().expect("helper");
    drop(parent);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_corrupt_output_record_is_skipped() {
    let path = temp_path("corrupt");
    let _ = std::fs::remove_file(&path);
    let region = MappedRegion::create(&path).expect("region");
    let offset = region.event_offset(1, true, 0);
    unsafe { region.map.as_ptr().cast_mut().add(offset).write(0xff) };
    region
        .slot_head(1)
        .output_events
        .store(1, Ordering::Release);
    let note = Event::NoteOn(NoteEvent {
        time: 0,
        track: 0,
        target: Target::note(1, 1),
        velocity: 1.0,
    });
    let mut decoded = [note];
    assert_eq!(region.read_output_events(1, &mut decoded), 0);
    drop(region);
    let _ = std::fs::remove_file(&path);
}

/// A plugin can overwrite the header. The host numbers its own blocks, so a
/// forged `published` neither picks generation 0 nor skips the helper's next
/// block (found by the region fuzz target, ADR 0290).
#[test]
fn a_forged_published_counter_does_not_number_the_hosts_blocks() {
    let region = MappedRegion::create_shared(SessionLayout::stereo()).expect("region");
    let silence = [0.0f32; 4];
    let input = [&silence[..], &silence[..]];
    // Each block completes before the next, so only `published` is forged.
    for (forged, expected) in [(u64::MAX, 1), (7, 2)] {
        region.header().published.store(forged, Ordering::Release);
        let generation = region.submit_block(4, &input, &[]).expect("submit");
        assert_eq!(generation, expected);
        assert_eq!(region.published(), expected, "the helper's copy");
        region.header().completed.store(expected, Ordering::Release);
    }
    region.header().published.store(u64::MAX, Ordering::Release);
    assert_eq!(region.submit_activation().expect("activation"), 3);
    region.reset_for_restart();
    assert_eq!(region.published(), 0);
    let generation = region
        .submit_block(4, &input, &[])
        .expect("submit after a restart");
    assert_eq!(generation, 1);
}
