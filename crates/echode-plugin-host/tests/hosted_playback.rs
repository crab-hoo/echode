//! Product playback through the test gain plugin. The helper is a separate process.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use echode_sandbox::{PluginHost, PluginNotice, SessionLayout, SpawnOptions};

use echode_engine::{LoadedClip, PlaybackCommand, PluginRoute, tick_to_sample};
use echode_model::{AudioRef, Id, NoteDraft, NoteGeometry, Project, Tick};

#[path = "hosted_playback/background_graph.rs"]
mod background_graph;
#[path = "hosted_playback/device_offline.rs"]
mod device_offline;
#[path = "hosted_playback/device_params.rs"]
mod device_params;
#[path = "hosted_playback/note_edges.rs"]
mod note_edges;
#[path = "hosted_playback/plugin_gui.rs"]
mod plugin_gui;
#[path = "hosted_playback/stopped_processing.rs"]
mod stopped_processing;

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"))
}

fn sine_route() -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.sine-pad".into(),
        path: host_bin()
            .parent()
            .unwrap()
            .join("plugins/EchodeSinePad.clap"),
        instrument: true,
        fault: None,
        state: None,
    }
}

fn with_sine(project: Project, track: Id) -> Project {
    project
        .add_clap_device(track, Id::generate(), "dev.echode.sine-pad")
        .unwrap()
}

fn playback_with_plugins(
    project: &Project,
    rate: u32,
    frames: usize,
    host: &Path,
    routes: &[PluginRoute],
) -> Result<
    (echode_engine::PlaybackControl, echode_engine::PlaybackAudio),
    echode_engine::PlaybackError,
> {
    let mut routes = routes.to_vec();
    routes.push(sine_route());
    echode_engine::playback_with_plugins(project, rate, frames, host, &routes)
}

fn playback_pair(
    project: &Project,
    rate: u32,
    frames: usize,
) -> Result<
    (echode_engine::PlaybackControl, echode_engine::PlaybackAudio),
    echode_engine::PlaybackError,
> {
    playback_with_plugins(project, rate, frames, &host_bin(), &[])
}

#[test]
fn contract_live_workgroup_failure_cannot_be_silent_offline_hosting() {
    let (project, track) = score();
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let group: echode_engine::PluginWorkgroup = Arc::new(move || {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("selected device workgroup unavailable".into())
    });
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: None,
        state: None,
    };
    let result = echode_engine::playback_with_plugins_in_workgroup_at(
        &project,
        48000,
        64,
        &host_bin(),
        &[route],
        0,
        group,
    );
    assert!(
        matches!(result, Err(echode_engine::PlaybackError::Unsupported(ref reason)) if reason.contains("selected device workgroup unavailable"))
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

fn headless_workgroup() -> echode_engine::RetainedWorkgroup {
    static LIBRARY: std::sync::OnceLock<libloading::Library> = std::sync::OnceLock::new();
    let library = LIBRARY.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/workgroup-fixture");
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("workgroup.dylib");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workgroup.c");
        assert!(
            std::process::Command::new("clang")
                .args([
                    "-dynamiclib",
                    "-arch",
                    "arm64",
                    "-mmacosx-version-min=27.0",
                    "-mcpu=apple-m4",
                    "-framework",
                    "AudioToolbox"
                ])
                .arg(source)
                .arg("-o")
                .arg(&binary)
                .status()
                .unwrap()
                .success()
        );
        // SAFETY: this fixture only calls public OS APIs; it contains no plugin.
        unsafe { libloading::Library::new(binary) }.unwrap()
    });
    // SAFETY: this is the fixture's own C function with exactly this signature.
    let create: libloading::Symbol<unsafe extern "C" fn() -> *mut std::ffi::c_void> =
        unsafe { library.get(b"echode_test_workgroup_create\0") }.unwrap();
    // SAFETY: the fixture returns one retained OS object. This test's period is
    // only a scheduling fixture, not a supported device or block-size limit.
    unsafe { echode_engine::RetainedWorkgroup::from_retained(create(), 32000) }.unwrap()
}

#[test]
fn live_workgroup_survives_product_edits_and_helper_restart() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // The 64-frame buffers belong only to this lifecycle regression.
    let owner = headless_workgroup();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let group: echode_engine::PluginWorkgroup = Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        owner.handoff()
    });
    let path = plugin_dylib();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.clone(),
        instrument: false,
        fault: None,
        state: None,
    };
    let (project, track) = Project::new().add_track("workgroup fixture");
    // Start without devices, then insert one after output preparation.
    let (mut control, mut audio) = echode_engine::playback_with_plugins_in_workgroup_at(
        &project,
        48000,
        64,
        &host_bin(),
        std::slice::from_ref(&route),
        0,
        Arc::clone(&group),
    )
    .unwrap();
    control.set_clip_loader(Arc::new(move |_, _| {
        // This source covers the device-free workgroup lifecycle fixture only.
        Ok(vec![LoadedClip {
            track: track.raw(),
            start_sample: 0,
            samples: vec![0.25; 4096].into(),
            right: None,
            fade_in: 0,
            fade_out: 0,
        }])
    }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let project = project
        .add_clap_device(track, Id::generate(), &route.clap_id)
        .unwrap();
    control.replace(&project).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().any(|sample| *sample != 0.0));
    control.service();
    control.replace(&project).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    control.service();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "unchanged helper was replaced"
    );
    let project = project
        .add_clap_device(track, Id::generate(), &route.clap_id)
        .unwrap();
    control.replace(&project).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    render_content(&mut audio, &mut left, &mut right);
    assert_eq!(control.status().plugin_missed_blocks, 0);

    let mut host = PluginHost::spawn(SpawnOptions {
        host_bin: &host_bin(),
        plugin: Some(&path),
        plugin_id: Some(&route.clap_id),
        workgroup: Some(group().unwrap()),
        fault: None,
        fan_in: None,
        layout: SessionLayout {
            max_frames: 64,
            input_channels: 2,
            output_channels: 2,
            input_events: 1,
            output_events: 1,
        },
        sample_rate: 48000,
    })
    .unwrap();
    host.restart().unwrap();
    assert!(host.stderr_log().is_empty(), "{}", host.stderr_log());
}

fn plugin_dylib() -> PathBuf {
    static ARTIFACT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ARTIFACT.get_or_init(build_plugin_dylib).clone()
}

fn build_plugin_dylib() -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let sibling = host_bin()
        .parent()
        .unwrap()
        .join("libechode_test_plugin.dylib");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../echode-test-plugin/src");
    let current = |path: &Path| {
        let Ok(built) = path.metadata().and_then(|meta| meta.modified()) else {
            return false;
        };
        let Ok(edited) = newest_source(&source) else {
            return false;
        };
        built >= edited
    };
    if current(&sibling) {
        return sibling;
    }
    let _guard = BUILD.lock().expect("plugin build lock");
    if current(&sibling) {
        return sibling;
    }
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugin");
    let status = std::process::Command::new(env!("CARGO"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(["build", "-p", "echode-test-plugin", "--target-dir"])
        .arg(&target)
        .status()
        .expect("build test plugin");
    assert!(status.success());
    target.join("debug").join("libechode_test_plugin.dylib")
}

#[test]
fn contract_macos_bundle_scan_to_playback_and_load_errors() {
    use echode_plugin_host::{cache_entry, resolve_cached, scan_out_of_process, store_scan_cache};
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(
        std::env::temp_dir().join(format!("echode-bundle-contract-{}", Id::generate().raw())),
    );
    let bundle = fixture.0.join("Test.clap");
    std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
    std::fs::copy(plugin_dylib(), bundle.join("Contents/MacOS/TestPlugin")).unwrap();
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>TestPlugin</string>
<key>CFBundleIdentifier</key><string>dev.echode.contract</string>
<key>CFBundlePackageType</key><string>BNDL</string></dict></plist>"#,
    )
    .unwrap();
    let scanned = scan_out_of_process(&host_bin(), &bundle).unwrap();
    let cache = fixture.0.join("scan.jsonl");
    store_scan_cache(&cache, &[cache_entry(&bundle, scanned)]).unwrap();
    assert!(bundle.is_dir() && !bundle.is_file());
    let (dry_project, track) = score();
    // Buffer lengths belong to this regression. The generous deadline is only
    // a hung-helper guard; realtime performance is measured by spike-isolation.
    let frames = 512;
    let (mut dry_control, mut dry) = playback_pair(&dry_project, 48000, frames).unwrap();
    dry_control.command(PlaybackCommand::Play).unwrap();
    let (mut expected, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    render_content(&mut dry, &mut expected, &mut right);
    assert!(expected.iter().any(|sample| *sample != 0.0));
    for id in ["dev.echode.test-gain", "dev.echode.test-instrument"] {
        let resolved = resolve_cached(&cache, &[id]);
        assert_eq!(resolved.len(), 1);
        let found = &resolved[0];
        assert_eq!(found.path, bundle);
        let route = PluginRoute {
            clap_id: found.clap_id.clone(),
            path: found.path.clone(),
            instrument: found.instrument,
            fault: None,
            state: None,
        };
        let project = dry_project
            .add_clap_device(track, Id::generate(), id)
            .unwrap();
        let (mut control, mut player) =
            playback_with_plugins(&project, 48000, frames, &host_bin(), &[route]).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        player.render_until(
            &mut left,
            &mut right,
            Instant::now() + Duration::from_secs(10),
        );
        assert!(!control.status().failed);
        assert!(control.plugin_failure().is_none());
        if found.instrument {
            assert!(
                left.iter().all(|sample| *sample == 0.0),
                "bundle instrument silently fell back to the built-in synth"
            );
        } else {
            let doubled: Vec<_> = expected.iter().map(|sample| sample * 2.0).collect();
            assert_close(&left, &doubled);
        }
    }
    let broken = fixture.0.join("Broken.clap");
    std::fs::create_dir_all(&broken).unwrap();
    let project = dry_project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: broken,
        instrument: false,
        fault: None,
        state: None,
    };
    assert!(
        playback_with_plugins(&project, 48000, frames, &host_bin(), &[route]).is_err(),
        "a broken installed bundle must not become a successful dry track"
    );
}

fn score() -> (Project, Id) {
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 64,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap()
        .0;
    (with_sine(project, track), track)
}

fn render(project: &Project, route: Option<PluginRoute>, frames: usize, blocks: usize) -> Vec<f32> {
    let (mut control, mut audio) = if let Some(route) = route {
        playback_with_plugins(project, 48_000, frames, &host_bin(), &[route]).unwrap()
    } else {
        playback_pair(project, 48_000, frames).unwrap()
    };
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    let mut heard = Vec::new();
    for _ in 0..blocks {
        audio.render(&mut left, &mut right);
        heard.extend_from_slice(&left);
        control.service();
    }
    heard
}

#[test]
#[ignore = "uses realtime block deadlines; make spike-isolation runs the release test"]
fn a_gain_plugin_doubles_the_builtin_synth_and_an_instrument_replaces_it() {
    let (project, track) = score();
    let with_gain = project
        .add_clap_device(track, Id::from_raw(90), "dev.echode.test-gain")
        .unwrap();
    let path = plugin_dylib();
    let effect = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.clone(),
        instrument: false,
        fault: None,
        state: None,
    };
    let dry = render(&project, None, 64, 4);
    let wet = render(&with_gain, Some(effect), 64, 4);
    assert!(dry.iter().any(|sample| *sample != 0.0));
    assert_eq!(dry.len(), wet.len());
    for (dry, wet) in dry.iter().zip(&wet) {
        assert!((wet - dry * 2.0).abs() < 1.0e-5, "{wet} != {dry} * 2");
    }
    let instrument = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path,
        instrument: true,
        fault: None,
        state: None,
    };
    let replaced = render(&with_gain, Some(instrument), 64, 2);
    assert!(replaced.iter().all(|sample| *sample == 0.0));
}

#[test]
#[ignore = "uses realtime block deadlines; make spike-isolation runs the release test"]
fn plugin_playback_does_not_depend_on_the_block_size() {
    let (project, track) = score();
    let project = project
        .add_clap_device(track, Id::from_raw(91), "dev.echode.test-gain")
        .unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: None,
        state: None,
    };
    // This is a timing-sensitive product integration check. Ordinary correctness
    // tests below use a hang guard instead of the realtime block deadline.
    let reference = render(&project, Some(route.clone()), 64, 1);
    for (frames, blocks) in [(8usize, 8usize), (16, 4), (32, 2)] {
        let heard = render(&project, Some(route.clone()), frames, blocks);
        assert_eq!(heard.len(), reference.len());
        assert_eq!(heard, reference);
    }
}

#[test]
fn plugin_output_is_independent_of_partition_without_a_realtime_deadline() {
    let (project, _) = score();
    let source = render(&project, None, 64, 1);
    let expected: Vec<_> = source.iter().map(|sample| sample * 2.0).collect();
    assert!(expected.iter().any(|sample| *sample != 0.0));
    let plugin = plugin_dylib();
    for frames in [1, 7, 8, 14, 28, 64] {
        let mut host = PluginHost::spawn(SpawnOptions {
            host_bin: &host_bin(),
            plugin: Some(&plugin),
            fault: None,
            workgroup: None,
            // The capacity covers this 64-sample fixture only, not a product limit.
            layout: SessionLayout {
                max_frames: 64,
                input_channels: 2,
                output_channels: 2,
                input_events: 1,
                output_events: 1,
            },
            fan_in: None,
            sample_rate: 48_000,
            plugin_id: Some("dev.echode.test-gain"),
        })
        .unwrap();
        let mut heard = Vec::new();
        for chunk in source.chunks(frames) {
            let mut left = vec![0.0; chunk.len()];
            let mut right = vec![0.0; chunk.len()];
            // This bounds a hung helper, not its realtime processing performance.
            host.process(
                &[chunk, chunk],
                &mut [&mut left, &mut right],
                Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(left, right);
            heard.extend(left);
        }
        assert_eq!(heard, expected, "partition {frames}");
    }
}

#[test]
fn audit_aud07_one_deadline_collects_a_healthy_track_beside_hangs() {
    let note = NoteDraft {
        pitch: 64,
        velocity: 100,
        start: Tick::new(0),
        length: Tick::quarters(2).unwrap(),
    };
    let (project, live) = Project::new().add_track("live");
    let mut project = project.insert_note(live, note).unwrap().0;
    let mut hang_ids = Vec::new();
    for name in ["hang-a", "hang-b", "hang-c"] {
        let (next, id) = project.add_track(name);
        project = next.insert_note(id, note).unwrap().0;
        hang_ids.push(id);
    }
    project = with_sine(project, live);
    let mut device = 90u128;
    project = project
        .add_clap_device(live, Id::from_raw(device), "dev.echode.test-gain")
        .unwrap();
    for id in hang_ids {
        device += 1;
        project = project
            .add_clap_device(id, Id::from_raw(device), "dev.echode.test-gain")
            .unwrap();
    }
    let path = plugin_dylib();
    let mut routes = vec![PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.clone(),
        instrument: false,
        fault: None,
        state: None,
    }];
    for _ in 0..3 {
        routes.push(PluginRoute {
            clap_id: "dev.echode.test-gain".into(),
            path: path.clone(),
            instrument: false,
            fault: Some("hang".into()),
            state: None,
        });
    }
    let (dry_project, dry_track) = Project::new().add_track("live");
    let dry_project = with_sine(
        dry_project.insert_note(dry_track, note).unwrap().0,
        dry_track,
    );
    let dry = render(&dry_project, None, 2048, 1);
    let expected: Vec<_> = dry.iter().map(|sample| sample * 2.0).collect();
    assert!(expected.iter().any(|sample| *sample != 0.0));

    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, 2048, &host_bin(), &routes).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; 2048];
    let mut right = vec![0.0; 2048];
    let started = std::time::Instant::now();
    audio.render(&mut left, &mut right);
    let elapsed = started.elapsed();
    // Four helpers that each waited out one 42.7 ms block took about 176 ms.
    // Sharing one deadline stays near a single block. 150 ms separates those
    // outcomes; it is not a performance budget.
    assert!(
        elapsed < Duration::from_millis(150),
        "four helpers stacked deadlines: {elapsed:?}"
    );
    for (heard, expected) in left.iter().zip(&expected) {
        assert!(
            (heard - expected).abs() < 1.0e-4,
            "{heard} != {expected}; a hang was mixed in or the live track was dropped"
        );
    }
    assert!(!control.status().failed);
    assert!(control.status().plugin_missed_blocks > 0);

    let started = std::time::Instant::now();
    audio.render_until(&mut left, &mut right, std::time::Instant::now());
    let spent = started.elapsed();
    assert!(
        spent < Duration::from_millis(30),
        "front-end work extended the hang deadline: {spent:?}"
    );
    assert!(!control.status().failed);
    assert!(control.status().plugin_missed_blocks > 0);
}

/// Content assertions need a hung-helper guard, not a realtime deadline.
/// The ignored deadline measurements and explicit hang/past-deadline probes
/// above still use the product's actual budget (ADR 0049).
fn render_content(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) {
    audio.render_until(left, right, Instant::now() + Duration::from_secs(10));
}

fn count_route(path: &Path) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.to_path_buf(),
        instrument: true,
        fault: Some("count".into()),
        state: None,
    }
}

fn level(events: f32) -> f32 {
    // The count fixture writes the same level into both channels of a stereo
    // main port. Center balance preserves that level in each channel.
    events
}

fn assert_level(heard: f32, events: f32) {
    let expected = level(events);
    assert!(
        (heard - expected).abs() < 1.0e-3,
        "{heard} != {expected} ({events} counted events)"
    );
}

#[test]
fn audit_aud05_notes_added_after_the_plugin_all_reach_it_and_its_state_stays() {
    let path = plugin_dylib();
    let route = count_route(&path);
    let frames = 256usize;
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .add_clap_device(track, Id::from_raw(90), "dev.echode.test-gain")
        .unwrap();
    let short = Tick::new(941);
    let off = tick_to_sample(short, project.tempo(), 48_000).unwrap();
    assert!(
        off > 0 && off < frames as i64,
        "note end {off} leaves the block"
    );
    let mut chord = project.clone();
    for pitch in [60u8, 64, 67] {
        chord = chord
            .insert_note(
                track,
                NoteDraft {
                    pitch,
                    velocity: 100,
                    start: Tick::new(0),
                    length: short,
                },
            )
            .unwrap()
            .0;
    }
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&route),
    )
    .unwrap();
    control.replace(&chord).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    render_content(&mut audio, &mut left, &mut right);
    // Three note-ons and three note-offs. The counter keeps both totals.
    assert_level(left[0], 3.0 + 3.0 * 16.0);
    assert!(!control.status().failed);

    let (project, track) = Project::new().add_track("pad");
    let project = project
        .add_clap_device(track, Id::from_raw(91), "dev.echode.test-gain")
        .unwrap();
    let long = Tick::quarters(2).unwrap();
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: long,
            },
        )
        .unwrap()
        .0;
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, frames, &host_bin(), &[route]).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert_level(left[0], 1.0);
    let mut grown = project;
    for pitch in [64u8, 67] {
        grown = grown
            .insert_note(
                track,
                NoteDraft {
                    pitch,
                    velocity: 100,
                    start: Tick::new(0),
                    length: long,
                },
            )
            .unwrap()
            .0;
    }
    control.replace(&grown).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    // The first note-on is still inside the plugin. Only the two new ones are sent.
    assert_level(left[0], 3.0);
    assert!(!control.status().failed);
}

#[test]
fn audit_pro04_growing_a_short_chord_keeps_every_note_off() {
    let path = plugin_dylib();
    let route = count_route(&path);
    let frames = 256usize;
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .add_clap_device(track, Id::from_raw(93), "dev.echode.test-gain")
        .unwrap();
    let short = Tick::new(941);
    let mut chord = project.clone();
    for pitch in [60u8, 64, 67] {
        chord = chord
            .insert_note(
                track,
                NoteDraft {
                    pitch,
                    velocity: 100,
                    start: Tick::new(0),
                    length: short,
                },
            )
            .unwrap()
            .0;
    }
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, frames, &host_bin(), &[route]).unwrap();
    control.replace(&chord).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    render_content(&mut audio, &mut left, &mut right);
    assert_level(left[0], 3.0 + 3.0 * 16.0);
    for pitch in [72u8, 76, 79] {
        chord = chord
            .insert_note(
                track,
                NoteDraft {
                    pitch,
                    velocity: 100,
                    start: Tick::new(0),
                    length: short,
                },
            )
            .unwrap()
            .0;
    }
    control.command(PlaybackCommand::Locate(0)).unwrap();
    control.replace(&chord).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    // Back at the start, all six notes start and end in this block. The counter
    // already holds the first three pairs, so the total is nine ons and nine offs
    // when none are dropped. Dropping offs leaves a smaller multiple of 16.
    assert_level(left[0], 9.0 + 9.0 * 16.0);
    assert!(!control.status().failed);
}

#[test]
fn audit_pro05_an_empty_track_keeps_the_held_note() {
    let path = plugin_dylib();
    let route = count_route(&path);
    let frames = 256usize;
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .add_clap_device(track, Id::from_raw(94), "dev.echode.test-gain")
        .unwrap();
    let long = Tick::quarters(2).unwrap();
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: long,
            },
        )
        .unwrap()
        .0;
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&route),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    render_content(&mut audio, &mut left, &mut right);
    assert_level(left[0], 1.0);
    let (project, _) = project.add_track("empty");
    control.replace(&project).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert_level(left[0], 1.0);
    assert!(!control.status().failed);
}

#[test]
fn audit_pro05_a_new_plugin_receives_the_held_note() {
    let path = plugin_dylib();
    let route = count_route(&path);
    let frames = 256usize;
    let (project, track) = Project::new().add_track("pad");
    let long = Tick::quarters(2).unwrap();
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: long,
            },
        )
        .unwrap()
        .0;
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, frames, &host_bin(), &[route]).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    render_content(&mut audio, &mut left, &mut right);
    let project = project
        .add_clap_device(track, Id::from_raw(95), "dev.echode.test-gain")
        .unwrap();
    control.replace(&project).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert_level(left[0], 1.0);
    assert!(!control.status().failed);
}

fn key_route(path: &Path) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.to_path_buf(),
        instrument: true,
        fault: Some("key".into()),
        state: None,
    }
}

fn hold_note(pitch: u8) -> (Project, Id, Id) {
    let (project, track) = Project::new().add_track("pad");
    let project = with_sine(project, track)
        .add_clap_device(track, Id::from_raw(92), "dev.echode.test-gain")
        .unwrap();
    let (project, note) = project
        .insert_note(
            track,
            NoteDraft {
                pitch,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap();
    (project, track, note)
}

fn first_sample(audio: &mut echode_engine::PlaybackAudio, frames: usize) -> f32 {
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    render_content(audio, &mut left, &mut right);
    left[0]
}

fn tail_player(
    path: &Path,
    frames: usize,
    chain: usize,
    fault: &str,
) -> (
    Project,
    Id,
    Id,
    echode_engine::PlaybackControl,
    echode_engine::PlaybackAudio,
) {
    let (mut project, track) = Project::new().add_track("tail regression");
    let (noted, note) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap();
    project = if chain == 1 {
        with_sine(noted, track)
    } else {
        noted
    };
    let mut routes = Vec::new();
    for (plugin, instrument, include) in [
        ("dev.echode.test-instrument", true, chain != 1),
        ("dev.echode.test-gain", false, chain != 0),
    ] {
        if !include {
            continue;
        }
        project = project
            .add_clap_device(track, Id::generate(), plugin)
            .unwrap();
        routes.push(PluginRoute {
            clap_id: plugin.into(),
            path: path.to_path_buf(),
            instrument,
            fault: Some(fault.into()),
            state: None,
        });
    }
    let (control, audio) =
        playback_with_plugins(&project, 48000, frames, &host_bin(), &routes).unwrap();
    (project, track, note, control, audio)
}

#[test]
fn contract_stop_seek_resume_does_not_replay_plugin_tails() {
    check_tail_lifecycle("tail");
}

#[test]
fn contract_midi_lifecycle_stops_all_declared_ports() {
    check_tail_lifecycle("tail-legacy");
}

fn check_tail_lifecycle(fault: &str) {
    let path = plugin_dylib();
    // Block sizes and three fixture chains only bound this regression's work.
    for frames in [64, 512] {
        for chain in 0..3 {
            for command in [PlaybackCommand::Stop, PlaybackCommand::Pause] {
                for coalesced in [false, true] {
                    let (_, _, _, mut control, mut audio) =
                        tail_player(&path, frames, chain, fault);
                    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
                    control.command(PlaybackCommand::Play).unwrap();
                    render_content(&mut audio, &mut left, &mut right);
                    assert!(left.iter().any(|sample| *sample != 0.0));
                    control.command(command).unwrap();
                    if !coalesced {
                        render_content(&mut audio, &mut left, &mut right);
                        assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
                    }
                    control.command(PlaybackCommand::Locate(480_000)).unwrap();
                    control.command(PlaybackCommand::Play).unwrap();
                    for _ in 0..3 {
                        render_content(&mut audio, &mut left, &mut right);
                        assert!(
                            left.iter().chain(&right).all(|sample| *sample == 0.0),
                            "old tail after {command:?}, chain={chain}, coalesced={coalesced}: {}",
                            left[0]
                        );
                    }
                    // Moving while playing clears old state but still chases a
                    // note which actually overlaps the destination.
                    control.command(PlaybackCommand::Locate(0)).unwrap();
                    render_content(&mut audio, &mut left, &mut right);
                    assert!(left.iter().any(|sample| *sample != 0.0));
                    control.command(PlaybackCommand::Locate(480_000)).unwrap();
                    render_content(&mut audio, &mut left, &mut right);
                    assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
                    assert!(control.plugin_failure().is_none());
                    assert!(!control.status().failed);
                }
            }
            // Stop/pause also clear tails when the position does not change.
            // Removing the note leaves the retained player at an empty point.
            for command in [
                Some(PlaybackCommand::Stop),
                Some(PlaybackCommand::Pause),
                None,
            ] {
                let (project, track, note, mut control, mut audio) =
                    tail_player(&path, frames, chain, fault);
                let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
                control.command(PlaybackCommand::Play).unwrap();
                render_content(&mut audio, &mut left, &mut right);
                if let Some(command) = command {
                    control.command(command).unwrap();
                    render_content(&mut audio, &mut left, &mut right);
                } else {
                    audio.render_gated_until(
                        &mut left,
                        &mut right,
                        Instant::now() + Duration::from_secs(10),
                        false,
                    );
                }
                control
                    .replace(&project.delete_note(track, note).unwrap())
                    .unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                render_content(&mut audio, &mut left, &mut right);
                assert!(
                    left.iter().chain(&right).all(|sample| *sample == 0.0),
                    "stop without locate: {command:?}, chain={chain}"
                );
                assert!(control.plugin_failure().is_none());
            }
            // A naturally ending note keeps its release, even though there is
            // no note at the new clock position. Muting empty regions is wrong.
            let (_, _, _, mut control, mut audio) = tail_player(&path, frames, chain, fault);
            let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
            control
                .command(PlaybackCommand::Locate(48_000 - frames as i64))
                .unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            for _ in 0..3 {
                render_content(&mut audio, &mut left, &mut right);
                assert!(
                    left.iter().any(|sample| *sample != 0.0),
                    "natural tail disappeared, chain={chain}"
                );
            }
        }
    }
}

#[test]
fn audit_aud06_delete_pitch_locate_loop_and_stop_release_the_plugin() {
    let path = plugin_dylib();
    let frames = 256usize;
    let long = Tick::quarters(2).unwrap();

    let (project, track, note) = hold_note(60);
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    let deleted = project.delete_note(track, note).unwrap();
    control.replace(&deleted).unwrap();
    // The note-on is still counted. The delete adds one note-off.
    assert_level(first_sample(&mut audio, frames), 1.0 + 16.0);
    assert!(!control.status().failed);

    let (project, _, _) = hold_note(60);
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    control.command(PlaybackCommand::Locate(96_000)).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0 + 16.0);
    assert!(!control.status().failed);

    let (project, track, note) = hold_note(60);
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    control.command(PlaybackCommand::Stop).unwrap();
    let silent = first_sample(&mut audio, frames);
    assert!(silent.abs() < 1.0e-6, "stop wrote {silent}");
    let deleted = project.delete_note(track, note).unwrap();
    control.replace(&deleted).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    // The stop callback delivered the note-off. The replacement plan has no note to send.
    assert_level(first_sample(&mut audio, frames), 1.0 + 16.0);
    assert!(!control.status().failed);

    let (project, track, note) = hold_note(60);
    let clip = project
        .tracks()
        .iter()
        .find(|found| found.id == track)
        .and_then(|found| found.clips.first())
        .unwrap()
        .id;
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    let retuned = project
        .edit_note(
            track,
            clip,
            note,
            NoteGeometry {
                pitch: 64,
                start: Tick::new(0),
                length: long,
            },
        )
        .unwrap();
    control.replace(&retuned).unwrap();
    // A pitch edit keeps the voice. No note-off and no second note-on.
    assert_level(first_sample(&mut audio, frames), 1.0);
    assert!(!control.status().failed);

    let (project, track, note) = hold_note(60);
    let clip = project
        .tracks()
        .iter()
        .find(|found| found.id == track)
        .and_then(|found| found.clips.first())
        .unwrap()
        .id;
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&key_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 0.0);
    let retuned = project
        .edit_note(
            track,
            clip,
            note,
            NoteGeometry {
                pitch: 64,
                start: Tick::new(0),
                length: long,
            },
        )
        .unwrap();
    control.replace(&retuned).unwrap();
    assert_level(first_sample(&mut audio, frames), 0.0);
    let deleted = retuned.delete_note(track, note).unwrap();
    control.replace(&deleted).unwrap();
    // The note-off uses the key the note started on, not the edited pitch.
    assert_level(first_sample(&mut audio, frames), 60.0);
    assert!(!control.status().failed);

    let (project, _, _) = hold_note(60);
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control
        .command(PlaybackCommand::SetLoop {
            start: 0,
            end: frames as i64,
        })
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    // The wrap releases the sounding note, then strikes the one at the loop head.
    assert_level(first_sample(&mut audio, frames), 2.0 + 16.0);
    assert!(!control.status().failed);
}

fn effect_route(path: &Path) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.to_path_buf(),
        instrument: false,
        fault: None,
        state: None,
    }
}

fn constant_clips(track: u128, value: f32, frames: usize) -> echode_engine::ClipLoader {
    let samples: Arc<[f32]> = vec![value; frames].into();
    Arc::new(move |_project, _rate| {
        Ok(vec![LoadedClip {
            track,
            start_sample: 0,
            samples: Arc::clone(&samples),
            right: None,
            fade_in: 0,
            fade_out: 0,
        }])
    })
}

fn play_left(
    project: &Project,
    route: Option<PluginRoute>,
    track: u128,
    clip: Option<f32>,
    frames: usize,
) -> Vec<f32> {
    let (mut control, mut audio) = if let Some(route) = route {
        playback_with_plugins(project, 48_000, frames, &host_bin(), &[route]).unwrap()
    } else {
        playback_pair(project, 48_000, frames).unwrap()
    };
    if let Some(value) = clip {
        control.set_clip_loader(constant_clips(track, value, frames));
        control.replace(project).unwrap();
    }
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    render_content(&mut audio, &mut left, &mut right);
    assert!(!control.status().failed);
    left
}

#[test]
fn contract_host_bypass_keeps_dry_audio_and_silences_instruments_under_rtsan() {
    let (project, track) = score();
    let frames = 64;
    let dry = play_left(&project, None, track.raw(), Some(0.25), frames);
    for instrument in [false, true] {
        let id = Id::from_raw(91);
        let mut route = effect_route(&plugin_dylib());
        // Bypass must not process even a discarded lifecycle block.
        route.fault = Some("process".into());
        if instrument {
            route.clap_id = "dev.echode.test-instrument".into();
            route.instrument = true;
        }
        let source = if instrument {
            project
                .remove_device(project.tracks()[0].devices[0].id)
                .unwrap()
        } else {
            project.clone()
        };
        let with = source.add_clap_device(track, id, &route.clap_id).unwrap();
        let bypassed = with.set_device_bypassed(id, true).unwrap();
        let heard = play_left(
            &bypassed,
            Some(route.clone()),
            track.raw(),
            Some(0.25),
            frames,
        );
        let expected = if instrument {
            vec![0.25 * std::f32::consts::FRAC_1_SQRT_2; frames]
        } else {
            dry.clone()
        };
        assert_close(&heard, &expected);
        if instrument {
            assert!(
                play_left(&bypassed, Some(route), track.raw(), None, frames)
                    .iter()
                    .all(|sample| *sample == 0.0)
            );
        }
    }
}

#[test]
fn bypassed_runs_preserve_stereo_audio_between_active_effects_under_rtsan() {
    let path = plugin_dylib();
    for flags in [
        [true, true, true],
        [false, true, false],
        [true, false, true],
    ] {
        for block in [7usize, 64] {
            // Fixture partitions, not a device/frame limit.
            let (mut project, track) = Project::new().add_track("bypass chain");
            for bypassed in flags {
                let device = Id::generate();
                project = project
                    .add_clap_device(track, device, "dev.echode.test-gain")
                    .unwrap()
                    .set_device_bypassed(device, bypassed)
                    .unwrap();
            }
            let (mut control, mut audio) =
                playback_with_plugins(&project, 48000, block, &host_bin(), &[effect_route(&path)])
                    .unwrap();
            control.set_clip_loader(Arc::new(move |_, _| {
                Ok(vec![LoadedClip {
                    track: track.raw(),
                    start_sample: 0,
                    samples: vec![0.125; 256].into(),
                    right: Some(vec![-0.25; 256].into()),
                    fade_in: 0,
                    fade_out: 0,
                }])
            }));
            control.replace(&project).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let gain = 2.0f32.powi(flags.into_iter().filter(|bypassed| !bypassed).count() as i32);
            let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
            for _ in 0..3 {
                render_content(&mut audio, &mut left, &mut right);
                assert!(!control.status().failed);
                assert!(control.plugin_failure().is_none());
                assert!(left.iter().all(|sample| *sample == 0.125 * gain));
                assert!(right.iter().all(|sample| *sample == -0.25 * gain));
                control.service();
            }
        }
    }
}

#[test]
fn reenabling_bypass_resets_stale_voices_and_chases_current_notes_under_rtsan() {
    for fault in ["tail", "tail-legacy"] {
        let (project, track, note, mut control, mut audio) =
            tail_player(&plugin_dylib(), 64, 0, fault);
        let device = project.tracks()[0].devices[0].id;
        control.command(PlaybackCommand::Play).unwrap();
        assert!(first_sample(&mut audio, 64) > 0.0);
        let bypassed = project.set_device_bypassed(device, true).unwrap();
        control.replace(&bypassed).unwrap();
        for _ in 0..8 {
            assert_eq!(first_sample(&mut audio, 64), 0.0);
            control.service();
        }
        control.replace(&project).unwrap();
        assert!(
            first_sample(&mut audio, 64) > 0.0,
            "current notes must be chased after resetting"
        );
        control.service();
        control.replace(&bypassed).unwrap();
        assert_eq!(first_sample(&mut audio, 64), 0.0);
        control.service();
        let removed = bypassed.delete_note(track, note).unwrap();
        control.replace(&removed).unwrap();
        assert_eq!(first_sample(&mut audio, 64), 0.0);
        control.service();
        control
            .replace(&removed.set_device_bypassed(device, false).unwrap())
            .unwrap();
        assert_eq!(
            first_sample(&mut audio, 64),
            0.0,
            "the helper must not resume its stale voice or tail"
        );
    }
}

fn assert_close(heard: &[f32], expected: &[f32]) {
    assert_eq!(heard.len(), expected.len());
    for (heard, expected) in heard.iter().zip(expected) {
        assert!((heard - expected).abs() < 1.0e-5, "{heard} != {expected}");
    }
}

#[test]
fn contract_stereo_planes_survive_effects_and_instrument_injection() {
    let path = plugin_dylib();
    // Deliberately asymmetric and opposite phase: duplicating either channel,
    // averaging to mono, or applying mono center pan cannot pass this fixture.
    for instrument in [false, true] {
        let (project, track) = Project::new().add_track("stereo chain");
        let project = project
            .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
            .unwrap()
            .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
            .unwrap();
        let mut first = effect_route(&path);
        first.instrument = instrument;
        let block = 64; // Scratch for this regression, not a channel/frame limit.
        let (mut control, mut audio) = playback_with_plugins(
            &project,
            48_000,
            block,
            &host_bin(),
            &[first, effect_route(&path)],
        )
        .unwrap();
        control.set_clip_loader(Arc::new(move |_, _| {
            Ok(vec![LoadedClip {
                track: track.raw(),
                start_sample: 0,
                samples: Arc::from(vec![0.125; 256]),
                right: Some(Arc::from(vec![-0.25; 256])),
                fade_in: 0,
                fade_out: 0,
            }])
        }));
        control.replace(&project).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        for _ in 0..3 {
            let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
            audio.render_until(
                &mut left,
                &mut right,
                Instant::now() + Duration::from_secs(10),
            );
            assert!(!control.status().failed);
            assert!(control.plugin_failure().is_none());
            let gain = if instrument { 2.0 } else { 4.0 };
            assert!(
                left.iter().all(|sample| *sample == 0.125 * gain),
                "left {left:?}"
            );
            assert!(
                right.iter().all(|sample| *sample == -0.25 * gain),
                "right {right:?}"
            );
            control.service();
        }
    }
}

#[test]
fn a_rejected_stereo_to_mono_mapping_preserves_the_sounding_helper() {
    let (project, track) = Project::new().add_track("mono instrument");
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-mono")
        .unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-mono".into(),
        path: plugin_dylib(),
        instrument: true,
        fault: None,
        state: None,
    };
    let block = 64; // This test's scratch size, not an engine limit.
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, block, &host_bin(), &[route]).unwrap();
    control.set_clip_loader(constant_clips(track.raw(), 0.125, 1024));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    audio.render_until(
        &mut left,
        &mut right,
        Instant::now() + Duration::from_secs(10),
    );
    let before = left.clone();
    assert!(
        before
            .iter()
            .all(|sample| (*sample - 0.375 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-5)
    );
    control.set_clip_loader(Arc::new(move |_, _| {
        Ok(vec![LoadedClip {
            track: track.raw(),
            start_sample: 0,
            samples: Arc::from(vec![0.125; 1024]),
            right: Some(Arc::from(vec![-0.125; 1024])),
            fade_in: 0,
            fade_out: 0,
        }])
    }));
    let error = control
        .replace(&project)
        .expect_err("implicit stereo downmix was accepted");
    assert!(error.to_string().contains("bus mapping"), "{error}");
    audio.render_until(
        &mut left,
        &mut right,
        Instant::now() + Duration::from_secs(10),
    );
    assert!(!control.status().failed);
    assert!(control.plugin_failure().is_none());
    assert_eq!(left, before);
    assert_eq!(right, before);
}

#[test]
fn rejected_unbypass_preserves_the_stereo_dry_graph_under_rtsan() {
    let (project, track) = Project::new().add_track("bypassed mono instrument");
    let device = Id::generate();
    let project = project
        .add_clap_device(track, device, "dev.echode.test-mono")
        .unwrap();
    let bypassed = project.set_device_bypassed(device, true).unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-mono".into(),
        path: plugin_dylib(),
        instrument: true,
        fault: None,
        state: None,
    };
    let block = 64; // Scratch for this regression, not an engine limit.
    let (mut control, mut audio) =
        playback_with_plugins(&bypassed, 48_000, block, &host_bin(), &[route]).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| {
        Ok(vec![LoadedClip {
            track: track.raw(),
            start_sample: 0,
            samples: vec![0.125; 1024].into(),
            right: Some(vec![-0.25; 1024].into()),
            fade_in: 0,
            fade_out: 0,
        }])
    }));
    control.replace(&bypassed).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    render_content(&mut audio, &mut left, &mut right);
    let before = (left.clone(), right.clone());
    assert_eq!(before, (vec![0.125; block], vec![-0.25; block]));
    let error = control
        .replace(&project)
        .expect_err("implicit stereo downmix was accepted");
    assert!(error.to_string().contains("bus mapping"), "{error}");
    render_content(&mut audio, &mut left, &mut right);
    assert_eq!((left.clone(), right.clone()), before);
    assert!(!control.status().failed);
    control.replace(&bypassed).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert_eq!((left, right), before);
}

#[test]
fn audit_aud08_clips_and_notes_reach_the_track_effect_before_the_fader() {
    let path = plugin_dylib();
    let frames = 256usize;
    let gain = effect_route(&path);
    let pan = std::f32::consts::FRAC_1_SQRT_2;
    // The clip's first and last samples are the edge fade. The body is the constant.
    let body = frames / 2;

    let (project, track) = Project::new().add_track("audio");
    let project = project
        .add_clap_device(track, Id::from_raw(93), "dev.echode.test-gain")
        .unwrap();
    let audio = AudioRef::try_new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        48_000,
        1,
        frames as u64,
    )
    .unwrap();
    let project = project
        .place_audio(track, Id::generate(), Id::generate(), Tick::new(0), audio)
        .unwrap();
    let dry_audio = play_left(&project, None, track.raw(), Some(0.25), frames);
    let wet_audio = play_left(
        &project,
        Some(gain.clone()),
        track.raw(),
        Some(0.25),
        frames,
    );
    // 0.25 through center pan, then the same clip after a gain of 2.
    assert!((dry_audio[body] - 0.25 * pan).abs() < 1.0e-5);
    assert!((wet_audio[body] - 0.35355338).abs() < 1.0e-5);
    let doubled: Vec<_> = dry_audio.iter().map(|sample| sample * 2.0).collect();
    assert_close(&wet_audio, &doubled);

    let (project, track) = score();
    let project = project
        .add_clap_device(track, Id::from_raw(94), "dev.echode.test-gain")
        .unwrap();
    let dry_notes = play_left(&project, None, track.raw(), None, frames);
    let wet_notes = play_left(&project, Some(gain.clone()), track.raw(), None, frames);
    let doubled: Vec<_> = dry_notes.iter().map(|sample| sample * 2.0).collect();
    assert_close(&wet_notes, &doubled);
    assert!(dry_notes.iter().any(|sample| *sample != 0.0));

    // A note and a loaded clip can sound in one block. The model still rejects
    // two placements that cover the same time, so this clip is not a second
    // placement; it is the audio the control thread hands to playback.
    let dry_both = play_left(&project, None, track.raw(), Some(0.25), frames);
    let wet_both = play_left(
        &project,
        Some(gain.clone()),
        track.raw(),
        Some(0.25),
        frames,
    );
    let doubled: Vec<_> = dry_both.iter().map(|sample| sample * 2.0).collect();
    assert_close(&wet_both, &doubled);
    assert!((dry_both[body] - (dry_notes[body] + 0.25 * pan)).abs() < 1.0e-4);

    let (project, track) = Project::new().add_track("instrument");
    let project = project
        .add_clap_device(track, Id::from_raw(95), "dev.echode.test-gain")
        .unwrap();
    let instrument = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.clone(),
        instrument: true,
        fault: None,
        state: None,
    };
    let replaced = play_left(&project, Some(instrument), track.raw(), Some(0.25), frames);
    // The instrument hears silence, not the clip. The clip is added to its output.
    assert_close(&replaced, &dry_audio);

    let (project, track) = score();
    let project = project
        .remove_device(project.tracks()[0].devices[0].id)
        .unwrap()
        .add_clap_device(track, Id::from_raw(96), "dev.echode.test-gain")
        .unwrap()
        .add_clap_device(track, Id::from_raw(97), "dev.echode.test-gain")
        .unwrap();
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        &[count_route(&path), gain],
    )
    .unwrap();
    control.set_clip_loader(constant_clips(track.raw(), 0.25, frames));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    render_content(&mut audio, &mut left, &mut right);
    assert!(!control.status().failed);
    // Count writes 1 and ignores its input. The clip is mixed onto that output,
    // then the gain effect doubles the sum.
    let expected = (1.0 + 0.25 * pan) * 2.0;
    assert!(
        (left[body] - expected).abs() < 1.0e-4,
        "{} != {expected}",
        left[body]
    );
}

fn spawn_declared(id: &str) -> Result<PluginHost, echode_sandbox::SessionError> {
    PluginHost::spawn(echode_sandbox::SpawnOptions {
        host_bin: &host_bin(),
        plugin: Some(plugin_dylib().as_path()),
        fault: None,
        workgroup: None,
        layout: SessionLayout {
            max_frames: 64,
            input_channels: 2,
            output_channels: 2,
            input_events: 4,
            output_events: 1,
        },
        fan_in: None,
        sample_rate: 48_000,
        plugin_id: Some(id),
    })
}

#[test]
fn audit_pro03_saved_state_changes_the_sound_and_survives_restart() {
    let mut host = spawn_declared("dev.echode.test-state").expect("state");
    let input = vec![0.0; 64];
    let mut left = vec![0.0; 64];
    let mut right = vec![0.0; 64];
    host.process(
        &[&input, &input],
        &mut [&mut left, &mut right],
        Duration::from_secs(2),
    )
    .expect("initial");
    assert!((left[0] - 0.25).abs() < 1.0e-4);
    assert_eq!(host.save_state().expect("save"), vec![64]);
    host.restore_state(&[128]).expect("restore");
    let started = Instant::now();
    loop {
        if matches!(
            host.poll_notice().expect("notice"),
            Some(PluginNotice::Dirty)
        ) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(2), "dirty notice");
        std::thread::sleep(Duration::from_millis(10));
    }
    host.process(
        &[&input, &input],
        &mut [&mut left, &mut right],
        Duration::from_secs(2),
    )
    .expect("restored");
    assert!((left[0] - 0.5).abs() < 1.0e-4, "{}", left[0]);
    assert_eq!(host.save_state().expect("save again"), vec![128]);
    host.request_reset();
    host.process(
        &[&input, &input],
        &mut [&mut left, &mut right],
        Duration::from_secs(2),
    )
    .expect("processing reset");
    assert!((left[0] - 0.5).abs() < 1.0e-4, "reset changed the preset");
    assert_eq!(host.save_state().unwrap(), vec![128]);
    host.restart().expect("restart");
    host.process(
        &[&input, &input],
        &mut [&mut left, &mut right],
        Duration::from_secs(2),
    )
    .expect("after restart");
    assert!((left[0] - 0.5).abs() < 1.0e-4, "{}", left[0]);

    let mut gain = spawn_declared("dev.echode.test-gain").expect("gain");
    let error = gain.save_state().expect_err("gain has no state");
    assert!(error.to_string().contains("state"), "{error}");

    let (project, track) = Project::new().add_track("state");
    let project = project
        .add_clap_device(track, Id::from_raw(91), "dev.echode.test-state")
        .unwrap();
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        64,
        &host_bin(),
        &[PluginRoute {
            clap_id: "dev.echode.test-state".into(),
            path: plugin_dylib(),
            instrument: false,
            fault: None,
            state: Some(vec![128]),
        }],
    )
    .expect("playback");
    control.command(PlaybackCommand::Play).unwrap();
    let mut heard = vec![0.0; 64];
    let mut silent = vec![0.0; 64];
    render_content(&mut audio, &mut heard, &mut silent);
    let expected = 0.5;
    assert!(
        (heard[32] - expected).abs() < 1.0e-3,
        "{} != {expected}",
        heard[32]
    );
    assert!(!control.status().failed);
}

#[test]
fn audit_pro02_declared_ports_are_what_process_sees() {
    let path = plugin_dylib();
    let mut mono = spawn_declared("dev.echode.test-mono").expect("mono");
    assert_eq!(mono.layout().input_channels, 0);
    assert_eq!(mono.layout().output_channels, 1);
    assert_eq!(mono.input_ports(), &[] as &[u32]);
    assert_eq!(mono.output_ports(), &[1]);
    let mut mono_out = vec![-1.0; 64];
    mono.process(&[], &mut [&mut mono_out], Duration::from_secs(2))
        .expect("mono process");
    assert!((mono_out[0] - 0.25).abs() < 1.0e-4);

    let mut ports = spawn_declared("dev.echode.test-ports").expect("ports");
    assert_eq!(ports.input_ports(), &[2]);
    assert_eq!(ports.output_ports(), &[2, 1]);
    let input = vec![0.25; 64];
    let mut left = vec![0.0; 64];
    let mut right = vec![0.0; 64];
    let mut extra = vec![0.0; 64];
    ports
        .process(
            &[&input, &input],
            &mut [&mut left, &mut right, &mut extra],
            Duration::from_secs(2),
        )
        .expect("ports process");
    assert!((left[0] - 0.5).abs() < 1.0e-4);
    assert!((right[0] - 0.5).abs() < 1.0e-4);
    assert!((extra[0] - 0.125).abs() < 1.0e-4);

    let Err(rejected) = spawn_declared("dev.echode.test-noports") else {
        panic!("a plugin without audio ports was accepted");
    };
    assert!(rejected.to_string().contains("audio port"), "{rejected}");

    let mut flip = spawn_declared("dev.echode.test-zflip").expect("flip");
    assert_eq!(flip.layout().input_channels, 2);
    assert_eq!(flip.layout().output_channels, 2);
    let mut flip_left = vec![0.0; 64];
    let mut flip_right = vec![0.0; 64];
    flip.process(
        &[&input, &input],
        &mut [&mut flip_left, &mut flip_right],
        Duration::from_secs(2),
    )
    .expect("stereo flip");
    assert!((flip_left[0] - 0.5).abs() < 1.0e-4);
    let flag = std::env::temp_dir().join("echode-pro02-port-flip");
    std::fs::write(&flag, b"mono").expect("flag");
    flip.restart().expect("restart");
    assert_eq!(flip.layout().input_channels, 0);
    assert_eq!(flip.layout().output_channels, 1);
    let mut flipped = vec![0.0; 64];
    flip.process(&[], &mut [&mut flipped], Duration::from_secs(2))
        .expect("mono flip");
    assert!((flipped[0] - 0.25).abs() < 1.0e-4);
    let _ = std::fs::remove_file(&flag);

    let (project, track) = Project::new().add_track("mono");
    let project = project
        .add_clap_device(track, Id::from_raw(70), "dev.echode.test-mono")
        .unwrap();
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        64,
        &host_bin(),
        &[PluginRoute {
            clap_id: "dev.echode.test-mono".into(),
            path,
            instrument: true,
            fault: None,
            state: None,
        }],
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut heard = vec![0.0; 64];
    let mut right = vec![0.0; 64];
    render_content(&mut audio, &mut heard, &mut right);
    let expected = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (heard[32] - expected).abs() < 1.0e-3,
        "{} != {expected}",
        heard[32]
    );
    assert!(!control.status().failed);

    let (project, track) = Project::new().add_track("rejected");
    let project = project
        .add_clap_device(track, Id::from_raw(71), "dev.echode.test-noports")
        .unwrap();
    let Err(error) = playback_with_plugins(
        &project,
        48_000,
        64,
        &host_bin(),
        &[PluginRoute {
            clap_id: "dev.echode.test-noports".into(),
            path: plugin_dylib(),
            instrument: false,
            fault: None,
            state: None,
        }],
    ) else {
        panic!("playback accepted a plugin without audio ports");
    };
    assert!(error.to_string().contains("audio port"), "{error}");
}

fn order_route() -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-order".into(),
        path: plugin_dylib(),
        instrument: true,
        fault: None,
        state: None,
    }
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().copied().fold(0.0, f32::max)
}

#[test]
fn audit_rea02_chased_notes_merge_before_future_events() {
    let (project, track) = Project::new().add_track("held");
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap()
        .0;
    let future = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 64,
                velocity: 100,
                start: Tick::new(8_820),
                length: Tick::new(100),
            },
        )
        .unwrap()
        .0;
    let sample = tick_to_sample(Tick::new(8_820), future.tempo(), 48_000).unwrap();
    assert_eq!(sample, 300);
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, 256, &host_bin(), &[order_route()]).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; 256];
    let mut right = vec![0.0; 256];
    render_content(&mut audio, &mut left, &mut right);
    let with_plugin = future
        .add_clap_device(track, Id::from_raw(999), "dev.echode.test-order")
        .unwrap();
    control.replace(&with_plugin).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert!(
        control.plugin_failure().is_none(),
        "{:?}",
        control.plugin_failure()
    );
    assert!(peak(&left) > 0.5, "chase missing: {}", peak(&left));
}

#[test]
fn audit_rea03_two_replacements_before_callback_keep_chase() {
    let (project, track) = Project::new().add_track("held");
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap()
        .0;
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, 256, &host_bin(), &[order_route()]).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; 256];
    let mut right = vec![0.0; 256];
    render_content(&mut audio, &mut left, &mut right);
    let with_plugin = project
        .add_clap_device(track, Id::from_raw(999), "dev.echode.test-order")
        .unwrap();
    control.replace(&with_plugin).unwrap();
    let with_empty = with_plugin.add_track("empty").0;
    control.replace(&with_empty).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert!(
        control.plugin_failure().is_none(),
        "{:?}",
        control.plugin_failure()
    );
    assert!(peak(&left) > 0.5, "chase dropped: {}", peak(&left));
    assert!(control.plugin_event_caps().iter().all(|cap| *cap < 64));
}

fn state_route(path: &Path, bytes: Option<Vec<u8>>) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-state".into(),
        path: path.to_path_buf(),
        instrument: false,
        fault: None,
        state: bytes,
    }
}

fn state_project() -> Project {
    let (project, track) = Project::new().add_track("state");
    project
        .add_clap_device(track, Id::from_raw(91), "dev.echode.test-state")
        .unwrap()
}

fn hear_state(
    control: &mut echode_engine::PlaybackControl,
    audio: &mut echode_engine::PlaybackAudio,
) -> f32 {
    let mut left = vec![0.0; 64];
    let mut right = vec![0.0; 64];
    render_content(audio, &mut left, &mut right);
    assert!(!control.status().failed);
    assert!(control.plugin_failure().is_none());
    left[32]
}

#[test]
fn audit_rea04_same_device_state_change_updates_sound() {
    let path = plugin_dylib();
    let project = state_project();
    let host = host_bin();
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        64,
        &host,
        &[state_route(&path, Some(vec![64]))],
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let quiet = 0.25;
    let loud = 0.5;
    let heard = hear_state(&mut control, &mut audio);
    assert!((heard - quiet).abs() < 1.0e-3, "start {heard}");
    control.set_routes(&host, &[state_route(&path, Some(vec![128]))]);
    control.replace(&project).unwrap();
    let heard = hear_state(&mut control, &mut audio);
    assert!((heard - loud).abs() < 1.0e-3, "128 {heard}");
    control.set_routes(&host, &[state_route(&path, Some(vec![64]))]);
    control.replace(&project).unwrap();
    let heard = hear_state(&mut control, &mut audio);
    assert!((heard - quiet).abs() < 1.0e-3, "back {heard}");
    control.set_routes(&host, &[state_route(&path, None)]);
    control.replace(&project).unwrap();
    let heard = hear_state(&mut control, &mut audio);
    assert!((heard - quiet).abs() < 1.0e-3, "cleared {heard}");
}

#[test]
fn audit_rea04_two_devices_keep_their_own_state() {
    let path = plugin_dylib();
    let (project, track) = Project::new().add_track("state");
    let project = project
        .add_clap_device(track, Id::from_raw(91), "dev.echode.test-state")
        .unwrap()
        .add_clap_device(track, Id::from_raw(92), "dev.echode.test-state")
        .unwrap();
    let host = host_bin();
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        64,
        &host,
        &[
            state_route(&path, Some(vec![64])),
            state_route(&path, Some(vec![128])),
        ],
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let quiet = 0.25;
    let loud = 0.5;
    let heard = hear_state(&mut control, &mut audio);
    assert!(
        (heard - loud).abs() < 1.0e-3,
        "the later device kept its own bytes: {heard}"
    );
    control.set_routes(
        &host,
        &[
            state_route(&path, Some(vec![128])),
            state_route(&path, Some(vec![64])),
        ],
    );
    control.replace(&project).unwrap();
    let heard = hear_state(&mut control, &mut audio);
    assert!(
        (heard - quiet).abs() < 1.0e-3,
        "swapped bytes stayed with each device: {heard}"
    );
}

#[test]
fn audit_rea05_dirty_reaches_durable_product_state() {
    use echode_actions::{Action, Command};
    let directory = std::env::temp_dir().join(format!(
        "echode-rea05-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    echode_history::write_package(&directory, &Project::new()).unwrap();
    let host = echode_history::DocumentHost::open(&directory).unwrap();
    let Action::AddTrack { id: track, .. } = host
        .perform(Command::AddTrack {
            name: "state".into(),
        })
        .unwrap()
    else {
        panic!("add track");
    };
    let Action::AddDevice { device, .. } = host
        .perform(Command::AddDevice {
            track,
            plugin: "dev.echode.test-state".into(),
        })
        .unwrap()
    else {
        panic!("add device");
    };
    let project = host.snapshot();
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        64,
        &host_bin(),
        &[state_route(&plugin_dylib(), Some(vec![128]))],
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; 64];
    let mut right = vec![0.0; 64];
    render_content(&mut audio, &mut left, &mut right);
    let started = Instant::now();
    let bytes = loop {
        control.service();
        if let Some((_, bytes)) = control.take_plugin_saves().into_iter().next() {
            break bytes;
        }
        assert!(started.elapsed() < Duration::from_secs(2), "dirty save");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(bytes, vec![128]);
    assert!(host.commit_plugin_state(device, &bytes).unwrap());
    drop(control);
    drop(audio);
    drop(host);
    let loaded = echode_history::read_package(&directory).unwrap();
    let saved = loaded
        .tracks()
        .iter()
        .flat_map(|track| track.devices.iter())
        .find(|item| item.id == device)
        .and_then(|item| item.state.clone())
        .expect("durable hash");
    assert_eq!(
        echode_history::read_object(&directory, &saved).unwrap(),
        vec![128]
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn routine_state_polling_preserves_continuous_hosted_audio_under_rtsan() {
    let (project, track) = Project::new().add_track("constant effect input");
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let path = plugin_dylib();
    // The block and run lengths bound this fixture only. There is no realtime
    // threshold here: render_content uses a hung-helper guard.
    let frames = 64;
    let blocks = 256;
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        96_000,
        frames,
        &host_bin(),
        &[effect_route(&path)],
    )
    .unwrap();
    control.set_clip_loader(constant_clips(track.raw(), 0.125, frames * blocks));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let worker = std::thread::spawn(move || {
        for _ in 0..blocks {
            worker_barrier.wait();
            control.service();
            worker_barrier.wait();
        }
        control
    });
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    let expected = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
    let mut wrong_samples = 0;
    for _ in 0..blocks {
        barrier.wait();
        render_content(&mut audio, &mut left, &mut right);
        barrier.wait();
        wrong_samples += left
            .iter()
            .chain(&right)
            .filter(|sample| (**sample - expected).abs() > 1.0e-7)
            .count();
    }
    let control = worker.join().unwrap();
    assert_eq!(wrong_samples, 0, "state polling changed the plugin route");
    assert_eq!(control.status().plugin_missed_blocks, 0);
    assert_eq!(control.status().graph_unavailable_blocks, 0);
}

#[test]
fn contract_preparing_a_graph_keeps_current_audio_running_under_rtsan() {
    use std::io::{Read, Write};
    let (project, track) = Project::new().add_track("continuous");
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let path = plugin_dylib();
    // These sizes bound only this fixture, not project or device capacity.
    let frames = 64;
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        96_000,
        frames,
        &host_bin(),
        &[effect_route(&path)],
    )
    .unwrap();
    control.set_clip_loader(constant_clips(track.raw(), 0.125, frames * 64));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    render_content(&mut audio, &mut left, &mut right);
    let before = control.status().sample;
    let socket_path = Path::new("/private/tmp").join(format!("ep-{}.sock", Id::generate().raw()));
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (changed, added) = project.add_track("initializing");
    let changed = changed
        .add_clap_device(added, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let mut waiting = effect_route(&path);
    waiting.fault = Some(format!("prepare-wait:{}", socket_path.display()));
    let worker = std::thread::spawn(move || {
        control.set_routes(&host_bin(), &[effect_route(&path), waiting]);
        let result = control.replace(&changed);
        (control, result)
    });
    // A hung-initializer guard, not a realtime performance threshold.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut peer = loop {
        match listener.accept() {
            Ok((peer, _)) => break peer,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(error) => panic!("initializer handshake: {error}"),
        }
    };
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reached = [0];
    peer.read_exact(&mut reached).unwrap();
    let expected = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
    let mut wrong = 0;
    for _ in 0..8 {
        render_content(&mut audio, &mut left, &mut right);
        wrong += left
            .iter()
            .chain(&right)
            .filter(|value| (**value - expected).abs() > 1.0e-7)
            .count();
    }
    peer.write_all(&[1]).unwrap();
    let (control, result) = worker.join().unwrap();
    std::fs::remove_file(socket_path).unwrap();
    result.unwrap();
    assert_eq!(wrong, 0, "graph preparation changed existing audio");
    assert_eq!(control.status().sample, before + (frames * 8) as i64);
    assert_eq!(control.status().graph_unavailable_blocks, 0);
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().all(|value| (*value - expected).abs() < 1.0e-7));
}

#[test]
fn queued_event_capacity_changes_preserve_the_existing_processor_under_rtsan() {
    let path = plugin_dylib();
    let (base, track) = Project::new().add_track("held");
    let base = base
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let base = base
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(16).unwrap(),
            },
        )
        .unwrap()
        .0;
    let frames = 64; // Fixture block length only.
    let (mut control, mut audio) =
        playback_with_plugins(&base, 48_000, frames, &host_bin(), &[count_route(&path)]).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    render_content(&mut audio, &mut left, &mut right);
    assert_level(left[0], 1.0);
    // Several edits arrive before audio adopts the first prepared mapping.
    // Future notes increase storage without changing the sounding note.
    for count in [4, 16, 2, 8, 0] {
        let mut next = base.clone();
        for offset in 0..count {
            next = next
                .insert_note(
                    track,
                    NoteDraft {
                        pitch: 61,
                        velocity: 100,
                        start: Tick::quarters(20 + offset).unwrap(),
                        length: Tick::quarters(1).unwrap(),
                    },
                )
                .unwrap()
                .0;
        }
        control.replace(&next).unwrap();
    }
    for _ in 0..16 {
        render_content(&mut audio, &mut left, &mut right);
        control.service();
        assert!(control.take_preparation_error().is_none());
        assert!(
            left.iter().all(|sample| (*sample - 1.0).abs() < 1.0e-7),
            "processor restarted or chased twice: {left:?}"
        );
    }
    assert!(!control.status().pending_plan);
    control.command(PlaybackCommand::Stop).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    // Mapping publication must also progress without any sounding audio.
    for count in [3, 9, 1] {
        let mut next = base.clone();
        for offset in 0..count {
            next = next
                .insert_note(
                    track,
                    NoteDraft {
                        pitch: 61,
                        velocity: 100,
                        start: Tick::quarters(20 + offset).unwrap(),
                        length: Tick::quarters(1).unwrap(),
                    },
                )
                .unwrap()
                .0;
        }
        control.replace(&next).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while control.status().pending_plan {
        render_content(&mut audio, &mut left, &mut right);
        control.service();
        assert!(control.take_preparation_error().is_none());
        assert!(Instant::now() < deadline, "stopped bank failed to activate");
        std::thread::yield_now();
    }
    assert_eq!(control.status().plugin_missed_blocks, 0);
    assert_eq!(control.status().graph_unavailable_blocks, 0);
}

#[test]
fn live_stop_releases_notes_and_locate_clears_tails_under_rtsan() {
    let path = plugin_dylib();
    let frames = 64; // This fixture's callback buffer, not a device limit.
    let (project, track) = score();
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 67,
                velocity: 100,
                start: Tick::new(echode_model::TICKS_PER_QUARTER / 100),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap()
        .0;
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, frames, &host_bin(), &[count_route(&path)])
            .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    for _ in 0..2 {
        audio.render_live_gated_until(
            &mut left,
            &mut right,
            Instant::now() + Duration::from_secs(5),
            true,
        );
    }
    assert!(
        control.plugin_failure().is_none(),
        "{:?}",
        control.plugin_failure()
    );
    assert_level(left[0], 1.0);
    let stopped = control.status().sample;
    control.command(PlaybackCommand::Stop).unwrap();
    for _ in 0..4 {
        audio.render_live_gated_until(
            &mut left,
            &mut right,
            Instant::now() + Duration::from_secs(5),
            true,
        );
        assert_eq!(control.status().sample, stopped);
        assert!(audio.stop_fade_offset().is_some());
        assert_level(left[0], 17.0); // One off, and no future note-on.
    }
    assert_eq!(control.status().plugin_missed_blocks, 0);
    for fault in ["tail", "tail-legacy"] {
        for chain in 0..3 {
            let (_, _, _, mut control, mut audio) = tail_player(&path, frames, chain, fault);
            control.command(PlaybackCommand::Play).unwrap();
            audio.render_live_gated_until(
                &mut left,
                &mut right,
                Instant::now() + Duration::from_secs(5),
                true,
            );
            assert!(left.iter().any(|value| *value != 0.0));
            control.command(PlaybackCommand::Stop).unwrap();
            for _ in 0..6 {
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    Instant::now() + Duration::from_secs(5),
                    true,
                );
            }
            assert!(
                left.iter().any(|value| *value != 0.0),
                "stop cut the natural tail"
            );
            control.command(PlaybackCommand::Locate(200_000)).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            while !control.status().playing {
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    Instant::now() + Duration::from_secs(5),
                    true,
                );
                assert!(
                    left.iter().chain(&right).all(|value| *value == 0.0),
                    "old {fault}/{chain} voice survived live stop"
                );
                assert!(!control.status().failed);
                assert!(
                    Instant::now() < until,
                    "cleanup did not finish for {fault}/{chain}"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            // Silence while cleanup is pending is insufficient. Inspect output
            // after the new play has advanced and every helper has processed it.
            for _ in 0..4 {
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    Instant::now() + Duration::from_secs(5),
                    true,
                );
                assert!(left.iter().chain(&right).all(|value| *value == 0.0));
            }
            assert!(control.status().sample > 200_000);
            assert!(control.plugin_failure().is_none());
        }
    }
}

#[test]
fn contract_live_discontinuity_cleanup_never_waits_for_a_reset() {
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    struct Gate(UnixStream, PathBuf);
    impl Drop for Gate {
        fn drop(&mut self) {
            let _ = self.0.write_all(&[1]);
            let _ = std::fs::remove_file(&self.1);
        }
    }
    // macOS sockaddr_un is shorter than its default per-user temp path.
    let socket_path =
        PathBuf::from("/tmp").join(format!("echode-reset-{}.sock", Id::generate().raw()));
    let listener = UnixListener::bind(&socket_path).unwrap();
    let path = plugin_dylib();
    let (project, track, _) = hold_note(60);
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path,
        instrument: false,
        fault: Some(format!("reset-wait:{}", socket_path.display())),
        state: None,
    };
    let frames = 512; // Matches the reported fixture, not a product buffer limit.
    let (mut control, mut audio) =
        playback_with_plugins(&project, 96_000, frames, &host_bin(), &[route]).unwrap();
    // hold_note has one effect; the fixture adds a second so cleanup must be
    // dispatched independently to both stages before either reset completes.
    let mut gates = Vec::new();
    for _ in 0..2 {
        let (peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        gates.push(Gate(peer, socket_path.clone()));
    }
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    let render = |audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]| {
        // A hung-helper guard, not a performance threshold. The held reset is
        // released only after this call returns, so waiting loses the receipt.
        audio.render_live_gated_until(left, right, Instant::now() + Duration::from_secs(2), true);
    };
    control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..8 {
        render(&mut audio, &mut left, &mut right);
    }
    assert!(left.iter().any(|sample| *sample != 0.0));
    let stopped = control.status().sample;
    // Stop preserves tails now; an explicit position change still clears old
    // voices without waiting for discarded processing on the callback thread.
    control.command(PlaybackCommand::Locate(stopped)).unwrap();
    render(&mut audio, &mut left, &mut right); // Cleanup cannot wait for reset.
    let mut reached = [0];
    gates[0].0.read_exact(&mut reached).unwrap();
    assert!(
        control.plugin_failure().is_none(),
        "discarded cleanup waited for a helper: {:?}",
        control.plugin_failure()
    );
    assert_eq!(control.status().plugin_missed_blocks, 0);
    assert_eq!(control.status().sample, stopped);
    assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
    gates[1].0.read_exact(&mut reached).unwrap();

    // A score update and a late worker-side pause must not retire the graph
    // that still owns these cleanup receipts, or restart either reset.
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Pause).unwrap();
    render(&mut audio, &mut left, &mut right);
    assert!(control.status().pending_plan);
    assert_eq!(control.status().sample, stopped);

    control.command(PlaybackCommand::Play).unwrap();
    render(&mut audio, &mut left, &mut right);
    assert_eq!(
        control.status().sample,
        stopped,
        "new notes advanced before cleanup finished"
    );
    assert!(!control.status().playing);
    assert!(control.plugin_failure().is_none());
    for gate in &mut gates {
        gate.0.write_all(&[1]).unwrap();
        gate.0.read_exact(&mut reached).unwrap();
        assert_eq!(reached, [2]);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !control.status().playing {
        render(&mut audio, &mut left, &mut right);
        assert!(
            !control.status().failed,
            "resume failed: {:?}, {:?}",
            control.status(),
            control.plugin_failure()
        );
        assert!(
            Instant::now() < deadline,
            "cleanup receipt did not release the pending start: {:?}, {:?}",
            control.status(),
            control.plugin_failure()
        );
        // Only the test driver waits between synthetic callbacks. Do not
        // monopolize a core while the separate helper publishes its receipt.
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(control.status().sample, stopped + frames as i64);
    for _ in 0..8 {
        render(&mut audio, &mut left, &mut right);
    }
    assert!(left.iter().any(|sample| *sample != 0.0));
    assert_eq!(
        control.status().plugin_missed_blocks,
        0,
        "{:?}, {:?}",
        control.status(),
        control.plugin_failure()
    );
    assert!(!control.status().pending_plan);
}

fn compare_note_onsets(path: &Path, clap_id: &str, fault: Option<&str>) {
    use echode_engine::OutputDeclick;
    use echode_model::TICKS_PER_QUARTER;
    use echode_realtime::Event;

    for rate in [48_000, 96_000] {
        let mut partition_reference = None;
        for frames in [7, 64, 512] {
            let (mut project, track) = Project::new().add_track("onset comparison");
            project = project
                .add_clap_device(track, Id::generate(), clap_id)
                .unwrap();
            let mut edges = Vec::new();
            for start in [TICKS_PER_QUARTER / 20, TICKS_PER_QUARTER / 8] {
                let length = TICKS_PER_QUARTER / 20;
                project = project
                    .insert_note(
                        track,
                        NoteDraft {
                            pitch: 60,
                            velocity: 100,
                            start: Tick::new(start),
                            length: Tick::new(length),
                        },
                    )
                    .unwrap()
                    .0;
                edges.push((
                    tick_to_sample(Tick::new(start), project.tempo(), rate).unwrap() as usize,
                    true,
                ));
                edges.push((
                    tick_to_sample(Tick::new(start + length), project.tempo(), rate).unwrap()
                        as usize,
                    false,
                ));
            }
            let route = PluginRoute {
                clap_id: clap_id.into(),
                path: path.into(),
                instrument: true,
                fault: fault.map(str::to_owned),
                state: None,
            };
            let (mut control, mut audio) =
                playback_with_plugins(&project, rate, frames, &host_bin(), &[route]).unwrap();
            let mut direct = PluginHost::spawn(SpawnOptions {
                host_bin: &host_bin(),
                plugin: Some(path),
                fault,
                workgroup: None,
                // Storage covers only this diagnostic block. Plugin port counts
                // are discovered by the helper, not fixed by this stereo seed.
                layout: SessionLayout {
                    max_frames: frames as u32,
                    ..SessionLayout::stereo()
                },
                fan_in: None,
                sample_rate: rate,
                plugin_id: Some(clap_id),
            })
            .unwrap();
            let inputs = vec![vec![0.0; frames]; direct.layout().input_channels as usize];
            let inputs: Vec<_> = inputs.iter().map(Vec::as_slice).collect();
            let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
            let (mut expected_left, mut expected_right) = (left.clone(), right.clone());
            let mut declick = OutputDeclick::new(rate);
            control.command(PlaybackCommand::Play).unwrap();
            let mut peak = 0.0f32;
            let mut onset_delta = 0.0f32;
            let mut previous = [0.0f32; 2];
            let mut heard = [Vec::new(), Vec::new()];
            // A tenth of a second includes two notes with silence between them.
            // This bounds the fixture, not the supported project duration.
            for start in (0..rate as usize / 10).step_by(frames) {
                let events: Vec<_> = edges
                    .iter()
                    .enumerate()
                    .filter_map(|(id, &(sample, on))| {
                        if !(start..start + frames).contains(&sample) {
                            return None;
                        }
                        let time = (sample - start) as u32;
                        let note_id = (id / 2) as u32;
                        Some(if on {
                            Event::note_on(
                                time,
                                0,
                                note_id,
                                60,
                                f64::from(echode_model::midi7_to_velocity(100)) / 65535.0,
                            )
                        } else {
                            Event::note_off(time, 0, note_id, 60)
                        })
                    })
                    .collect();
                direct
                    .process_block(
                        frames,
                        &inputs,
                        &mut [&mut expected_left, &mut expected_right],
                        &events,
                        &mut [],
                        Duration::from_secs(10),
                    )
                    .unwrap();
                // Only a hung-helper guard. This is a content comparison and
                // makes no claim about device deadlines or audible hardware.
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    Instant::now() + Duration::from_secs(10),
                    true,
                );
                // The same live output join used by HAL. No new transport
                // command occurs when a note begins after ordinary silence.
                declick.process(&mut left, &mut right, start == 0);
                let kept = frames.min(rate as usize / 10 - start);
                heard[0].extend_from_slice(&left[..kept]);
                heard[1].extend_from_slice(&right[..kept]);
                assert_eq!(
                    left, expected_left,
                    "left altered: {clap_id}, {rate} Hz, {frames} frames, at {start}"
                );
                assert_eq!(
                    right, expected_right,
                    "right altered: {clap_id}, {rate} Hz, {frames} frames, at {start}"
                );
                for (index, (&l, &r)) in left.iter().zip(&right).enumerate() {
                    peak = peak.max(l.abs()).max(r.abs());
                    if edges.iter().any(|&(sample, on)| {
                        on && (sample..sample + rate as usize / 200).contains(&(start + index))
                    }) {
                        onset_delta = onset_delta
                            .max((l - previous[0]).abs())
                            .max((r - previous[1]).abs());
                    }
                    previous = [l, r];
                    if start + index < edges[0].0 {
                        assert_eq!([l, r], [0.0, 0.0]);
                    }
                }
                assert!(
                    control.plugin_failure().is_none(),
                    "{:?}",
                    control.plugin_failure()
                );
            }
            assert!(
                peak > 0.0,
                "a silent fixture cannot prove onset transparency"
            );
            assert_eq!(control.status().plugin_missed_blocks, 0);
            assert!(!control.status().failed);
            if let Some(reference) = &partition_reference {
                assert_eq!(
                    &heard, reference,
                    "partition changed the test voice at {rate} Hz / {frames} frames"
                );
            } else {
                partition_reference = Some(heard);
            }
            println!(
                "{clap_id}: {rate} Hz / {frames} frames, stereo sample error=0, peak={peak}, first 5 ms max delta={onset_delta}"
            );
        }
    }
}

#[test]
fn contract_plugin_note_onsets_preserve_source_samples_under_rtsan() {
    compare_note_onsets(&plugin_dylib(), "dev.echode.test-instrument", Some("onset"));
}

/// Newest edit among the test plugin's sources, so a change to any module rebuilds it.
fn newest_source(dir: &Path) -> std::io::Result<std::time::SystemTime> {
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    for entry in std::fs::read_dir(dir)? {
        newest = newest.max(entry?.metadata()?.modified()?);
    }
    Ok(newest)
}
