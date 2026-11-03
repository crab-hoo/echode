//! Release measurement of the callback's own work as tracks grow (XCA-13).
//!
//! Each plugin track runs the test gain in its own helper process. The figure
//! that matters is the callback thread's CPU time, which leaves out the time it
//! sleeps waiting for helpers. Work that grows with the square of the track
//! count shows up as CPU time per track that rises with the count.
//!
//! It also counts the descriptors the parent holds once the helpers are
//! running. An app started from the Finder begins with a soft limit of 256.
//!
//! No audio device is opened. `make spike-plugin-tracks` runs this and writes
//! `target/spike-plugin-tracks.json`. It is not a correctness test.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use echode_engine::{PlaybackCommand, PlaybackOptions, PluginRoute};
use echode_model::{Id, NoteDraft, Project, TICKS_PER_QUARTER, Tick};

const RATE: u32 = 48_000;
const FRAMES: usize = 128;
const WARMUP: usize = 200;
const BLOCKS: usize = 2_000;
/// Longer than the measured blocks, so every block keeps sounding notes.
const BEATS: i64 = 48;

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"))
}

fn gain_route() -> PluginRoute {
    let path = host_bin()
        .parent()
        .unwrap()
        .join("libechode_test_plugin.dylib");
    assert!(
        path.exists(),
        "{} is missing; make spike-plugin-tracks builds it",
        path.display()
    );
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path,
        instrument: false,
        fault: None,
        state: None,
    }
}

/// Every track gets a four-note chord on each beat, so a downbeat block holds
/// four note-ons per track.
fn project(tracks: usize, plugins: bool, chords: bool) -> Project {
    let mut project = Project::new();
    for index in 0..tracks {
        let (next, track) = project.add_track(format!("t{index}"));
        project = next;
        if chords {
            for beat in 0..BEATS {
                for pitch in [60, 64, 67, 71] {
                    project = project
                        .insert_note(
                            track,
                            NoteDraft {
                                pitch,
                                velocity: 100,
                                start: Tick::quarters(beat).unwrap(),
                                length: Tick::new(TICKS_PER_QUARTER / 2),
                            },
                        )
                        .unwrap()
                        .0;
                }
            }
        }
        if plugins {
            project = project
                .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
                .unwrap();
        }
    }
    project
}

fn thread_cpu_ns() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid, writable timespec for the call's duration.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
    assert_eq!(rc, 0, "thread CPU clock");
    now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64
}

fn open_descriptors() -> usize {
    std::fs::read_dir("/dev/fd").unwrap().count()
}

struct Sample {
    scenario: &'static str,
    tracks: usize,
    wall: Vec<u64>,
    cpu: Vec<u64>,
    missed: u64,
    /// Descriptors the parent opened for this transport and its helpers.
    descriptors: usize,
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

fn measure(scenario: &'static str, tracks: usize, plugins: bool, chords: bool) -> Sample {
    let project = project(tracks, plugins, chords);
    let routes = [gain_route()];
    let host = host_bin();
    let options = if plugins {
        PlaybackOptions::default().plugins(&host, &routes)
    } else {
        PlaybackOptions::default()
    };
    let before = open_descriptors();
    let (mut control, mut audio) =
        echode_engine::playback(&project, RATE, FRAMES, options).unwrap();
    let descriptors = open_descriptors().saturating_sub(before);
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; FRAMES];
    let mut right = vec![0.0; FRAMES];
    let mut wall = Vec::with_capacity(BLOCKS);
    let mut cpu = Vec::with_capacity(BLOCKS);
    for block in 0..WARMUP + BLOCKS {
        // A generous deadline keeps every helper's work in the figure. A
        // product callback would silence the late ones instead.
        let deadline = Instant::now() + Duration::from_secs(1);
        let started = Instant::now();
        let started_cpu = thread_cpu_ns();
        audio.render_until(&mut left, &mut right, deadline);
        let spent_cpu = thread_cpu_ns() - started_cpu;
        let spent = started.elapsed();
        if block >= WARMUP {
            wall.push(spent.as_nanos() as u64);
            cpu.push(spent_cpu);
        }
        control.service();
    }
    let status = control.status();
    assert!(!status.failed, "{scenario} {tracks}: playback failed");
    wall.sort_unstable();
    cpu.sort_unstable();
    Sample {
        scenario,
        tracks,
        wall,
        cpu,
        missed: status.plugin_missed_blocks,
        descriptors,
    }
}

#[test]
#[ignore = "release measurement; make spike-plugin-tracks runs it"]
fn measure_callback_cost_as_tracks_grow() {
    let mut samples = Vec::new();
    for tracks in [1, 16, 64, 256] {
        samples.push(measure("plugins", tracks, true, false));
    }
    for tracks in [1, 16, 64, 256] {
        samples.push(measure("chords", tracks, false, true));
    }
    let mut rows = Vec::new();
    println!(
        "scenario tracks | wall p50 p99 max us | cpu p50 p99 max us | cpu p50/track ns | missed | fds"
    );
    for sample in &samples {
        let us = |ns: u64| ns as f64 / 1_000.0;
        let (wall, cpu) = (&sample.wall, &sample.cpu);
        let per_track = percentile(cpu, 0.5) as f64 / sample.tracks as f64;
        println!(
            "{} {} | {:.1} {:.1} {:.1} | {:.1} {:.1} {:.1} | {:.0} | {} | {}",
            sample.scenario,
            sample.tracks,
            us(percentile(wall, 0.5)),
            us(percentile(wall, 0.99)),
            us(*wall.last().unwrap()),
            us(percentile(cpu, 0.5)),
            us(percentile(cpu, 0.99)),
            us(*cpu.last().unwrap()),
            per_track,
            sample.missed,
            sample.descriptors,
        );
        rows.push(serde_json::json!({
            "scenario": sample.scenario,
            "tracks": sample.tracks,
            "wall_us": {"p50": us(percentile(wall, 0.5)), "p99": us(percentile(wall, 0.99)), "max": us(*wall.last().unwrap())},
            "cpu_us": {"p50": us(percentile(cpu, 0.5)), "p99": us(percentile(cpu, 0.99)), "max": us(*cpu.last().unwrap())},
            "cpu_p50_per_track_ns": per_track,
            "missed_blocks": sample.missed,
            "parent_descriptors": sample.descriptors,
        }));
    }
    let report = serde_json::json!({
        "rate": RATE,
        "frames": FRAMES,
        "blocks": BLOCKS,
        "warmup": WARMUP,
        "rows": rows,
    });
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/spike-plugin-tracks.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
}
