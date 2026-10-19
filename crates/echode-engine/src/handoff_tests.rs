use super::{Audio, Control, Step, pair};
use echode_realtime::{Event, MixerKey, NoteSpan, Plan, TrackPlan};

fn blank() -> [Event; 8] {
    [Event::note_off(0, 0, 0, 0); 8]
}

fn gain_plan(gain: f32) -> Plan {
    Plan {
        tracks: vec![TrackPlan {
            track: 1,
            track_key: 0,
            notes: Vec::new(),
            by_off: Vec::new(),
            gain,
            pan: 0.0,
            mute: 0.0,
            solo: 0.0,
        }],
    }
}

fn noted(note: u128, on: i64, off: i64, pitch: u8) -> Plan {
    Plan {
        tracks: vec![TrackPlan {
            track: 1,
            track_key: 0,
            notes: vec![NoteSpan {
                on,
                off,
                note,
                note_id: note as u32,
                pitch,
                velocity: 100,
            }],
            by_off: vec![0],
            gain: 1.0,
            pan: 0.0,
            mute: 0.0,
            solo: 0.0,
        }],
    }
}

/// Pushes without draining, so a test can fill the return ring.
fn push_now(control: &mut Control, plan: Plan) {
    let package = super::Package::new(plan);
    if let Err(rtrb::PushError::Full(package)) = control.plans.push(package) {
        if control.held.is_some() {
            control.freed += 1;
        }
        control.held = Some(package);
    }
}

#[test]
fn a_full_return_ring_leaves_the_new_plan_queued() {
    let (mut control, mut audio) = pair();
    let mut edges = blank();
    push_now(&mut control, gain_plan(1.0));
    audio.callback(0, 0, &mut edges).unwrap();
    push_now(&mut control, gain_plan(2.0));
    audio.callback(0, 0, &mut edges).unwrap();
    push_now(&mut control, gain_plan(3.0));
    audio.callback(0, 0, &mut edges).unwrap();
    push_now(&mut control, gain_plan(4.0));
    let stalled = audio.callback(0, 0, &mut edges).unwrap();
    assert_eq!(stalled.step, Step::Running);
    assert_eq!(audio.plan().unwrap().tracks[0].gain, 3.0);
    control.set_mixer(1, MixerKey::Gain, 0.25);
    control.flush_params();
    audio.callback(0, 0, &mut edges).unwrap();
    assert_eq!(audio.plan().unwrap().tracks[0].gain, 0.25);
    control.service();
    audio.callback(0, 0, &mut edges).unwrap();
    assert_eq!(audio.plan().unwrap().tracks[0].gain, 4.0);
}

#[test]
fn replacing_a_sounding_note_emits_one_off_and_a_pitch_change_does_not() {
    let (mut control, mut audio) = pair();
    let mut edges = blank();
    push_now(&mut control, noted(7, 0, 1_000, 60));
    let first = audio.callback(100, 4, &mut edges).unwrap();
    assert_eq!(first.releases, 0);
    push_now(&mut control, noted(8, 0, 1_000, 67));
    let removed = audio.callback(100, 4, &mut edges).unwrap();
    assert_eq!(removed.releases, 1);
    assert!(edges[0].is_note_off());
    assert_eq!(edges[0].note_id(), Some(7));
    assert_eq!(edges[0].key(), Some(60));
    assert_eq!(edges[0].time(), 4);
    assert_eq!(edges[0].track_key(), Some(0));

    let (mut control, mut audio) = pair();
    push_now(&mut control, noted(7, 0, 1_000, 60));
    audio.callback(100, 4, &mut edges).unwrap();
    push_now(&mut control, noted(7, 0, 1_000, 64));
    let pitched = audio.callback(100, 4, &mut edges).unwrap();
    assert_eq!(pitched.releases, 0);
}

#[test]
fn mixer_writes_collapse_to_the_latest_value() {
    let (mut control, mut audio) = pair();
    let mut edges = blank();
    for value in [0.1, 0.2, 0.3] {
        control.set_mixer(1, MixerKey::Gain, value);
    }
    assert_eq!(control.pending.len(), 1);
    push_now(&mut control, gain_plan(1.0));
    audio.callback(0, 0, &mut edges).unwrap();
    control.flush_params();
    audio.callback(0, 0, &mut edges).unwrap();
    assert_eq!(audio.plan().unwrap().tracks[0].gain, 0.3);
    assert!(control.pending.is_empty());
}

#[test]
fn stop_returns_the_current_plan_and_leaves_the_audio_end_idle() {
    let (mut control, audio) = pair();
    let running = std::thread::spawn(move || run_until_stopped(audio));
    push_now(&mut control, gain_plan(1.0));
    while control.installed() == 0 {
        std::thread::yield_now();
    }
    control.request_stop();
    while !control.stopped() {
        control.service();
        std::thread::yield_now();
    }
    let audio = running.join().unwrap();
    assert!(audio.is_idle());
    control.service();
    assert_eq!(control.freed(), 1);
    drop(audio);
}

#[test]
fn ten_thousand_plan_swaps_run_under_rtsan() {
    let (mut control, audio) = pair();
    let running = std::thread::spawn(move || run_until_stopped(audio));
    for index in 0..10_000 {
        let plan = if index % 2 == 0 {
            noted(7, 0, 1_000, 60)
        } else {
            gain_plan(1.0)
        };
        control.publish(plan);
        control.set_mixer(1, MixerKey::Gain, 0.5);
        while control.installed() < index + 1 {
            control.service();
            std::thread::yield_now();
        }
    }
    control.request_stop();
    while !control.stopped() {
        control.service();
        std::thread::yield_now();
    }
    let audio = running.join().unwrap();
    assert!(audio.is_idle());
    assert_eq!(control.installed(), 10_000);
    control.service();
    assert_eq!(control.freed(), 10_000);
    drop(audio);
    drop(control);
}

fn run_until_stopped(mut audio: Audio) -> Audio {
    let mut edges = blank();
    loop {
        let cycle = realtime_callback(&mut audio, &mut edges);
        if cycle.step == Step::Stopped {
            return audio;
        }
    }
}

#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
fn realtime_callback(audio: &mut Audio, edges: &mut [Event]) -> super::Cycle {
    audio.callback(100, 0, edges).unwrap()
}
