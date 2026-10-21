use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use echode_actions::{Command, Session};
use echode_cli::{
    WAV_FAULT_DIR_SYNC, WAV_FAULT_FLUSH, WAV_FAULT_RENAME, WAV_FAULT_WRITE, arm_wav_fault,
    export_lines_for_test, render_to_wav, write_wav,
};
use echode_engine::{PlaybackCommand, playback_pair};
use echode_history::write_package;
use echode_model::{Id, NoteDraft, Project, Tick};

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("echode-render-{}-{n}", std::process::id()));
    // A failed earlier run can leave a package under a reused pid. Its
    // journal would replay into this test.
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn reference_project() -> Project {
    let (project, track) = Project::new().add_track("piano");
    project
        .add_clap_device(track, Id::generate(), "dev.echode.sine-pad")
        .unwrap()
        .insert_note(
            track,
            NoteDraft {
                pitch: 69,
                velocity: 127,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0
}

fn reference_wav() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine-pad-a4-128.wav")
}

fn render_reference(directory: &std::path::Path) -> Vec<u8> {
    write_package(directory, &reference_project()).unwrap();
    let wav = directory.join("out.wav");
    render_to_wav(directory, &wav, 48_000, Some(128)).unwrap();
    std::fs::read(wav).unwrap()
}

#[test]
fn a_package_renders_to_the_committed_reference() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    let rendered = render_reference(&directory);
    let reference = std::fs::read(reference_wav()).unwrap_or_else(|_| {
        panic!(
            "missing {}; run make bless REASON=\"why the sound changed\"",
            reference_wav().display()
        )
    });
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(&rendered[0..4], b"RIFF");
    assert_eq!(&rendered[8..12], b"WAVE");
    assert_eq!(&rendered[20..22], &3u16.to_le_bytes());
    assert!(rendered.len() > 44);
    assert!(rendered[44..].iter().any(|byte| *byte != 0));
    assert_eq!(rendered, reference);
}

#[test]
fn default_export_uses_the_declared_device_tail_and_respects_bypass() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    let project = reference_project();
    let device = project.tracks()[0].devices[0].id;
    for (index, project, tail) in [
        (0, project.clone(), 19_200usize),
        (1, project.set_device_bypassed(device, true).unwrap(), 0),
        (2, project.remove_device(device).unwrap(), 0),
    ] {
        let package = directory.join(index.to_string());
        write_package(&package, &project).unwrap();
        let wav = package.join("tail.wav");
        render_to_wav(&package, &wav, 48_000, None).unwrap();
        assert_eq!(
            std::fs::metadata(wav).unwrap().len() as usize,
            44 + (24_000 + tail) * 8
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_zero_sample_rate_and_a_negative_frame_count_leave_the_output() {
    let directory = scratch();
    write_package(&directory, &reference_project()).unwrap();
    let wav = directory.join("out.wav");
    std::fs::write(&wav, b"keep").unwrap();
    for frames in [None, Some(0), Some(16)] {
        let error = render_to_wav(&directory, &wav, 0, frames).unwrap_err();
        assert!(error.to_string().contains("sample rate"), "{error}");
        assert_eq!(std::fs::read(&wav).unwrap(), b"keep");
    }
    let parsed = echode_cli::run([
        "render",
        directory.to_str().unwrap(),
        wav.to_str().unwrap(),
        "--frames",
        "-1",
    ])
    .unwrap_err();
    assert!(
        parsed.to_string().contains("non-negative integer"),
        "{parsed}"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_placed_file_is_audible_in_the_wav() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("tone.wav");
    let mut tone = vec![0.0f32; 256];
    for (index, sample) in tone.iter_mut().enumerate() {
        *sample = if index % 4 < 2 { 0.25 } else { -0.25 };
    }
    write_wav(&source, 48_000, &tone).unwrap();
    let stored = echode_history::import_audio(&directory, &source).unwrap();
    let (project, track) = Project::new().add_track("file");
    let audio = echode_model::AudioRef::try_new(
        stored.hash,
        stored.sample_rate,
        stored.channels,
        stored.frames,
    )
    .unwrap();
    let project = project
        .place_audio(
            track,
            echode_model::Id::generate(),
            echode_model::Id::generate(),
            Tick::new(0),
            audio,
        )
        .unwrap();
    write_package(&directory, &project).unwrap();
    let wav = directory.join("out.wav");
    render_to_wav(&directory, &wav, 48_000, Some(128)).unwrap();
    let rendered = std::fs::read(&wav).unwrap();
    let _ = std::fs::remove_dir_all(directory);
    assert!(rendered.len() > 44);
    assert!(rendered[44..].iter().any(|byte| *byte != 0));
}

#[test]
#[ignore = "timing measurement; make spike-render runs it in a release build"]
fn a_minute_of_stereo_silence_is_written_in_bulk() {
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let wav = directory.join("minute.wav");
    let samples = vec![0.0f32; 48_000 * 60 * 2];
    let started = Instant::now();
    write_wav(&wav, 48_000, &samples).unwrap();
    let elapsed = started.elapsed();
    let bytes = std::fs::metadata(&wav).unwrap().len();
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(bytes, 44 + samples.len() as u64 * 4);
    // A write of one sample at a time took 15.3 s. Buffered writes of this
    // buffer were 0.028 s. Two seconds still fails the old path.
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "60 s stereo write took {elapsed:?}"
    );
}

/// Placement gain is linear on the prepared samples. Fades are zero so the
/// body is constant on each side. Stereo center balance preserves each channel.
#[test]
fn audit_aud09_clip_gain_scales_playback_export_undo_and_recovery() {
    const FRAMES: usize = 128;
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("constant.wav");
    let stereo: Vec<_> = (0..FRAMES).flat_map(|_| [0.25, -0.5]).collect();
    write_wav(&source, 48_000, &stereo).unwrap();
    let stored = echode_history::import_audio(&directory, &source).unwrap();
    let (project, track) = Project::new().add_track("file");
    let clip = Id::generate();
    let audio = echode_model::AudioRef::try_new(
        stored.hash,
        stored.sample_rate,
        stored.channels,
        stored.frames,
    )
    .unwrap();
    let project = project
        .place_audio(track, clip, Id::generate(), Tick::new(0), audio)
        .unwrap()
        .set_clip_fades(clip, 0, 0)
        .unwrap();

    let unity = play_middle(&directory, &project, FRAMES);
    let expected = 0.25;
    assert!(
        (unity - expected).abs() < 1e-4,
        "gain 1 playback {unity} expected {expected}"
    );
    for (gain, factor) in [(0.0_f32, 0.0), (0.5, 0.5), (1.0, 1.0), (2.0, 2.0)] {
        let gained = project.set_clip_gain(clip, gain).unwrap();
        let played = play_middle(&directory, &gained, FRAMES);
        let exported = export_middle(&directory, &gained, clip, FRAMES);
        let target = unity * factor;
        assert!(
            (played - target).abs() < 1e-4,
            "gain {gain} playback {played} expected {target}"
        );
        assert!(
            (exported - target).abs() < 1e-4,
            "gain {gain} export {exported} expected {target}"
        );
    }

    let mut session = Session::open(project);
    session
        .perform(Command::SetClipGain { clip, gain: 2.0 })
        .unwrap();
    let doubled = play_middle(&directory, session.project(), FRAMES);
    session.undo().unwrap();
    let restored = play_middle(&directory, session.project(), FRAMES);
    session.redo().unwrap();
    let redone = play_middle(&directory, session.project(), FRAMES);
    let _ = std::fs::remove_dir_all(directory);
    assert!(
        (doubled - unity * 2.0).abs() < 1e-4,
        "redo source {doubled}"
    );
    assert!((restored - unity).abs() < 1e-4, "undo {restored}");
    assert!((redone - unity * 2.0).abs() < 1e-4, "redo {redone}");
}

fn play_middle(package: &Path, project: &Project, frames: usize) -> f32 {
    let (mut control, mut audio) = playback_pair(project, 48_000, frames).unwrap();
    let package = package.to_path_buf();
    control.set_clip_loader(Arc::new(move |project, rate| {
        echode_audio_io::load_placed_clips(&package, project, rate)
    }));
    control.replace(project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    audio.render(&mut left, &mut right);
    assert!(left.iter().zip(&right).all(|(l, r)| *r == -2.0 * *l));
    left[frames / 2]
}

fn export_middle(package: &Path, project: &Project, clip: Id, frames: usize) -> f32 {
    write_package(package, project).unwrap();
    // An earlier export in this directory committed the previous gain. This
    // write replaces the whole package, so that tip is no longer the version.
    let _ = std::fs::remove_dir_all(package.join(".git"));
    let _ = std::fs::remove_file(package.join("journal.jsonl"));
    let _ = std::fs::remove_file(package.join("save.pending"));
    let recovered = echode_history::recover(package).unwrap();
    assert!((clip_gain(&recovered, clip) - clip_gain(project, clip)).abs() < 1e-6);
    let wav = package.join("out.wav");
    render_to_wav(package, &wav, 48_000, Some(u32::try_from(frames).unwrap())).unwrap();
    let bytes = std::fs::read(wav).unwrap();
    let start = 44 + (frames / 2) * 8;
    let left = f32::from_le_bytes(bytes[start..start + 4].try_into().unwrap());
    let right = f32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap());
    assert_eq!(right, -2.0 * left, "the WAV writer lost the right channel");
    left
}

#[test]
fn audit_aud11_a_journaled_note_exports_and_a_pending_save_is_not_mixed() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    write_package(&directory, &Project::new()).unwrap();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    let track = session.project().tracks()[0].id;
    session
        .perform(Command::AddDevice {
            track,
            plugin: "dev.echode.sine-pad".into(),
        })
        .unwrap();
    session
        .perform(Command::InsertNote {
            track,
            draft: NoteDraft {
                pitch: 69,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        })
        .unwrap();
    for (index, entry) in session.journal().iter().enumerate() {
        echode_history::append_journal(
            &directory,
            &echode_actions::JournalLine {
                seq: index as u64 + 1,
                op: echode_actions::JournalOp::Edit,
                actions: entry.actions.clone(),
                changes: entry.changes.clone().unwrap_or_default(),
            },
        )
        .unwrap();
    }
    let raw = echode_history::read_package(&directory).unwrap();
    assert!(raw.tracks().is_empty());

    let wav = directory.join("out.wav");
    render_to_wav(&directory, &wav, 48_000, Some(128)).unwrap();
    let exported = std::fs::read(&wav).unwrap();
    assert!(exported[44..].iter().any(|byte| *byte != 0));
    let host = echode_history::DocumentHost::open(&directory).unwrap();
    let revision = host.revisions().unwrap()[0].id.clone();
    assert!(!revision.is_empty());
    assert_eq!(host.snapshot().tracks()[0].name, "piano");
    drop(host);

    let dirty = scratch();
    std::fs::create_dir_all(&dirty).unwrap();
    write_package(&dirty, &Project::new()).unwrap();
    let opened = echode_history::DocumentHost::open(&dirty).unwrap();
    let committed = opened.revisions().unwrap()[0].id.clone();
    drop(opened);
    let (noted, track) = Project::new().add_track("dirty");
    let noted = noted
        .insert_note(
            track,
            NoteDraft {
                pitch: 69,
                velocity: 127,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0;
    write_package(&dirty, &noted).unwrap();
    echode_history::mark_save_pending(&dirty).unwrap();
    let dirty_wav = dirty.join("out.wav");
    render_to_wav(&dirty, &dirty_wav, 48_000, Some(128)).unwrap();
    let silent = std::fs::read(&dirty_wav).unwrap();
    assert!(silent[44..].iter().all(|byte| *byte == 0));
    let again = echode_history::DocumentHost::open(&dirty).unwrap();
    assert_eq!(again.revisions().unwrap()[0].id, committed);
    assert!(again.snapshot().tracks().is_empty());
    drop(again);

    let unfinished = scratch();
    std::fs::create_dir_all(&unfinished).unwrap();
    write_package(&unfinished, &noted).unwrap();
    echode_history::mark_save_pending(&unfinished).unwrap();
    let unfinished_wav = unfinished.join("out.wav");
    let error = render_to_wav(&unfinished, &unfinished_wav, 48_000, Some(128)).unwrap_err();
    assert!(!unfinished_wav.exists());
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(dirty);
    let _ = std::fs::remove_dir_all(unfinished);
    assert!(error.to_string().contains("unfinished"), "{error}");
}

static PLUGIN_ENV: Mutex<()> = Mutex::new(());

/// Same block the exporter uses for one plugin callback.
const PLUGIN_FRAMES: usize = 256;

#[test]
fn contract_phase1_project_survives_crash_history_and_cli_export() {
    use echode_history::DocumentHost;
    use std::io::{BufRead, Write};
    use std::process::{Command as Process, Stdio};

    const CHILD: &str = "ECHODE_P1_CRASH_PACKAGE";
    const READY: &str = "ECHODE_P1_EDITS_CONFIRMED";
    if let Some(directory) = std::env::var_os(CHILD) {
        let host = DocumentHost::open(Path::new(&directory)).unwrap();
        host.perform(Command::AddTrack {
            name: "phase1".into(),
        })
        .unwrap();
        let track = host.snapshot().tracks()[0].id;
        host.perform(Command::AddDevice {
            track,
            plugin: "dev.echode.test-instrument".into(),
        })
        .unwrap();
        host.perform(Command::InsertNote {
            track,
            draft: NoteDraft {
                pitch: 69,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        })
        .unwrap();
        println!("{READY}");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }

    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let cache = directory.join("plugins.jsonl");
    write_plugin_cache(&cache, &plugin_dylib());
    let _env = PluginEnv::set(&cache);
    // The fixture's note-driven stereo attack makes silence or a fallback
    // synth distinguishable from actual isolated CLAP playback.
    let _onset = FaultFile::set("dev.echode.test-instrument", "onset");
    let mut child = Process::new(std::env::current_exe().unwrap())
        .args([
            "contract_phase1_project_survives_crash_history_and_cli_export",
            "--exact",
            "--nocapture",
        ])
        .env(CHILD, &directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sent, ready) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if line.unwrap().trim() == READY {
                let _ = sent.send(());
            }
        }
    });
    // Hang watchdog, not a performance assertion. Kill only our fixture child,
    // after acknowledged edits, without DocumentHost's orderly shutdown.
    let confirmed = ready.recv_timeout(std::time::Duration::from_secs(15));
    let killed = child.kill();
    let output = child.wait_with_output().unwrap();
    reader.join().unwrap();
    assert!(
        confirmed.is_ok(),
        "child did not confirm edits: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    killed.unwrap();
    assert!(!output.status.success());

    let host = DocumentHost::open(&directory).unwrap();
    let original = host.snapshot();
    assert_eq!(original.tracks().len(), 1);
    assert_eq!(original.tracks()[0].name, "phase1");
    assert!(
        original.tracks()[0]
            .devices
            .iter()
            .any(|device| device.plugin.as_deref() == Some("dev.echode.test-instrument"))
    );
    assert_eq!(original.contents()[0].notes.len(), 1);
    assert_eq!(original.contents()[0].notes[0].pitch, 69);
    // Force a saved checkpoint after recovering the acknowledged journal.
    host.perform(Command::AddTrack {
        name: "checkpoint".into(),
    })
    .unwrap();
    host.undo().unwrap();
    assert_eq!(host.snapshot(), original);
    let revision = host.revisions().unwrap()[0].id.clone();
    let played = play_plugins(&directory, &original);
    assert!(played.iter().any(|sample| sample.abs() > 0.1));
    drop(host);

    let export = |name: &str| {
        let wav = directory.join(name);
        let output = Process::new(env!("CARGO_BIN_EXE_echode"))
            .arg("render")
            .arg(&directory)
            .arg(&wav)
            .args([
                "--sample-rate",
                "48000",
                "--frames",
                &PLUGIN_FRAMES.to_string(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(wav_lefts(&wav, PLUGIN_FRAMES), played);
        std::fs::read(wav).unwrap()
    };
    let reference = export("before.wav");
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack {
        name: "later".into(),
    })
    .unwrap();
    host.perform(Command::Restore {
        revision: revision.clone(),
    })
    .unwrap();
    assert_eq!(host.snapshot(), original);
    let revisions = host.revisions().unwrap();
    assert_ne!(revisions[0].id, revision);
    assert!(revisions.iter().any(|item| item.id == revision));
    host.undo().unwrap();
    assert_eq!(host.snapshot().tracks().len(), 2);
    host.redo().unwrap();
    assert_eq!(host.snapshot(), original);
    drop(host);
    assert_eq!(export("restored.wav"), reference);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn bypass_export_matches_dry_effects_and_silent_instruments() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let cache = directory.join("clap-scan.jsonl");
    let _env = PluginEnv::set(&cache);
    write_plugin_cache(&cache, &plugin_dylib());
    let project = reference_project();
    write_package(&directory, &project).unwrap();
    let wav = directory.join("out.wav");
    render_to_wav(&directory, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap();
    let dry = wav_lefts(&wav, PLUGIN_FRAMES);
    assert!(dry.iter().any(|sample| *sample != 0.0));
    let track = project.tracks()[0].id;
    for (plugin, instrument) in [
        ("dev.echode.test-gain", false),
        ("dev.echode.test-instrument", true),
    ] {
        let device = Id::generate();
        let source = if instrument {
            project
                .remove_device(project.tracks()[0].devices[0].id)
                .unwrap()
        } else {
            project.clone()
        };
        let bypassed = source
            .add_clap_device(track, device, plugin)
            .unwrap()
            .set_device_bypassed(device, true)
            .unwrap();
        let package = directory.join(if instrument { "instrument" } else { "effect" });
        write_package(&package, &bypassed).unwrap();
        let wav = package.join("out.wav");
        render_to_wav(&package, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap();
        let exported = wav_lefts(&wav, PLUGIN_FRAMES);
        assert_eq!(exported, play_plugins(&package, &bypassed));
        if instrument {
            assert!(exported.iter().all(|sample| *sample == 0.0));
        } else {
            assert_eq!(exported, dry);
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn audit_aud10_plugin_export_matches_playback_and_names_a_missing_plugin() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let _env = PluginEnv::set(&directory.join("clap-scan.jsonl"));
    let plugin = plugin_dylib();
    write_plugin_cache(&directory.join("clap-scan.jsonl"), &plugin);
    let source = directory.join("constant.wav");
    write_wav(&source, 48_000, &vec![0.25f32; PLUGIN_FRAMES * 2]).unwrap();
    let stored = echode_history::import_audio(&directory, &source).unwrap();
    let (project, track) = Project::new().add_track("file");
    let clip = Id::generate();
    let audio = echode_model::AudioRef::try_new(
        stored.hash,
        stored.sample_rate,
        stored.channels,
        stored.frames,
    )
    .unwrap();
    let project = project
        .place_audio(track, clip, Id::generate(), Tick::new(0), audio)
        .unwrap()
        .set_clip_fades(clip, 0, 0)
        .unwrap()
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0
        .add_clap_device(track, Id::from_raw(1), "dev.echode.test-instrument")
        .unwrap()
        .add_clap_device(track, Id::from_raw(2), "dev.echode.test-gain")
        .unwrap();
    write_package(&directory, &project).unwrap();

    let played = play_plugins(&directory, &project);
    let wav = directory.join("out.wav");
    render_to_wav(&directory, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap();
    let exported = wav_lefts(&wav, PLUGIN_FRAMES);
    let expected = 0.25 * 2.0;
    assert_eq!(played, exported);
    assert!(
        played
            .iter()
            .all(|sample| (*sample - expected).abs() < 1e-4),
        "body {} expected {expected}",
        played[PLUGIN_FRAMES / 2]
    );

    let missing = project
        .add_clap_device(track, Id::from_raw(3), "dev.echode.not-installed")
        .unwrap();
    let missing_dir = scratch();
    std::fs::create_dir_all(&missing_dir).unwrap();
    write_package(&missing_dir, &missing).unwrap();
    let error = render_to_wav(
        &missing_dir,
        &missing_dir.join("out.wav"),
        48_000,
        Some(PLUGIN_FRAMES as u32),
    )
    .unwrap_err();
    let _ = std::fs::remove_dir_all(&missing_dir);
    assert!(
        error.to_string().contains("dev.echode.not-installed"),
        "{error}"
    );

    write_plugin_cache(
        &directory.join("clap-scan.jsonl"),
        &directory.join("gone.clap"),
    );
    write_package(&directory, &project).unwrap();
    let error = render_to_wav(&directory, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap_err();
    assert!(
        error.to_string().contains("dev.echode.test-instrument"),
        "{error}"
    );
    assert!(
        error.to_string().contains("dev.echode.test-gain"),
        "{error}"
    );

    let _ = std::fs::remove_dir_all(&directory);
    drop(_env);
    let hostless = scratch();
    std::fs::create_dir_all(&hostless).unwrap();
    write_package(&hostless, &project).unwrap();
    write_plugin_cache(&hostless.join("clap-scan.jsonl"), &plugin);
    let _hostless = PluginEnv::without_host(&hostless.join("clap-scan.jsonl"));
    let error = render_to_wav(
        &hostless,
        &hostless.join("out.wav"),
        48_000,
        Some(PLUGIN_FRAMES as u32),
    )
    .unwrap_err();
    let _ = std::fs::remove_dir_all(hostless);
    assert!(error.to_string().contains("plugin host"), "{error}");
}

#[test]
fn audit_pro07_a_failed_plugin_does_not_leave_a_finished_wav() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let _env = PluginEnv::set(&directory.join("clap-scan.jsonl"));
    let plugin = plugin_dylib();
    write_plugin_cache(&directory.join("clap-scan.jsonl"), &plugin);
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0
        .add_clap_device(track, Id::from_raw(41), "dev.echode.test-gain")
        .unwrap();
    write_package(&directory, &project).unwrap();
    for (fault, needle) in [
        ("generate", "not found"),
        ("activate", "activate"),
        ("process", "process refused"),
        ("crash", "plugin process has exited"),
        ("hang", "did not finish before the deadline"),
    ] {
        let _fault = FaultFile::set("dev.echode.test-gain", fault);
        let wav = directory.join(format!("{fault}.wav"));
        let error =
            render_to_wav(&directory, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("41"), "{fault}: {text}");
        assert!(text.contains(needle), "{fault}: {text}");
        assert!(!wav.exists(), "{fault} left a wav");
    }

    // The first export commits the one-device project. A second write into that
    // package is hidden by the autosave tip, so the later device gets its own package.
    let later = project
        .add_clap_device(track, Id::from_raw(42), "dev.echode.test-instrument")
        .unwrap();
    let later_dir = scratch();
    std::fs::create_dir_all(&later_dir).unwrap();
    write_package(&later_dir, &later).unwrap();
    let _fault = FaultFile::set("dev.echode.test-instrument", "process");
    let wav = later_dir.join("later.wav");
    let error = render_to_wav(&later_dir, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("42"), "{text}");
    assert!(text.contains("dev.echode.test-instrument"), "{text}");
    // A quiet machine sees the process error. A busy one can reach the block
    // deadline first. Either way the later device is the one that failed.
    assert!(
        text.contains("process refused") || text.contains("did not finish before the deadline"),
        "{text}"
    );
    assert!(!text.contains("device 41"), "{text}");
    assert!(!wav.exists());
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&later_dir);
    drop(_env);
}

#[test]
fn audit_rea06_failed_plugin_export_preserves_existing_output() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let _env = PluginEnv::set(&directory.join("clap-scan.jsonl"));
    let plugin = plugin_dylib();
    write_plugin_cache(&directory.join("clap-scan.jsonl"), &plugin);
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0
        .add_clap_device(track, Id::from_raw(41), "dev.echode.test-gain")
        .unwrap();
    write_package(&directory, &project).unwrap();
    let prior = b"existing-finished-wav".to_vec();
    let wav = directory.join("out.wav");
    std::fs::write(&wav, &prior).unwrap();
    let lines = export_lines_for_test();
    let _fault = FaultFile::set("dev.echode.test-gain", "process");
    let error = render_to_wav(&directory, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap_err();
    let text = error.to_string();
    assert!(
        text.contains("process refused") || text.contains("did not finish before the deadline"),
        "{text}"
    );
    assert!(!error.is_durability_unknown());
    assert_eq!(std::fs::read(&wav).unwrap(), prior);
    assert_eq!(export_lines_for_test(), lines);
    assert!(part_files(&directory).is_empty());

    let fresh = scratch();
    std::fs::create_dir_all(&fresh).unwrap();
    write_package(&fresh, &project).unwrap();
    write_plugin_cache(&fresh.join("clap-scan.jsonl"), &plugin);
    let missing = fresh.join("new.wav");
    let error = render_to_wav(&fresh, &missing, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap_err();
    assert!(error.to_string().contains("41"), "{error}");
    assert!(!missing.exists());
    assert!(part_files(&fresh).is_empty());
    drop(_fault);

    let lines = export_lines_for_test();
    for (kind, skip) in [
        (WAV_FAULT_WRITE, 1u32),
        (WAV_FAULT_FLUSH, 0),
        (WAV_FAULT_RENAME, 0),
    ] {
        std::fs::write(&wav, &prior).unwrap();
        arm_wav_fault(kind, skip);
        let error =
            render_to_wav(&directory, &wav, 48_000, Some((PLUGIN_FRAMES * 2) as u32)).unwrap_err();
        assert!(!error.is_durability_unknown(), "{kind}: {error}");
        assert_eq!(std::fs::read(&wav).unwrap(), prior, "{kind}");
        assert!(part_files(&directory).is_empty(), "{kind}");
    }
    assert_eq!(export_lines_for_test(), lines);
    arm_wav_fault(0, 0);

    std::fs::write(&wav, &prior).unwrap();
    arm_wav_fault(WAV_FAULT_DIR_SYNC, 0);
    let error = render_to_wav(&directory, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap_err();
    assert!(error.is_durability_unknown(), "{error}");
    assert!(
        error.to_string().contains("durability is unknown"),
        "{error}"
    );
    let published = std::fs::read(&wav).unwrap();
    assert_ne!(published, prior);
    assert_eq!(&published[0..4], b"RIFF");
    assert_eq!(export_lines_for_test(), lines);
    arm_wav_fault(0, 0);

    let replaced = export_lines_for_test();
    arm_wav_fault(0, 0);
    render_to_wav(&directory, &wav, 48_000, Some(PLUGIN_FRAMES as u32)).unwrap();
    let done = std::fs::read(&wav).unwrap();
    assert_eq!(&done[0..4], b"RIFF");
    assert_ne!(done, prior);
    assert!(export_lines_for_test() > replaced);

    std::fs::write(&wav, &prior).unwrap();
    let lines = export_lines_for_test();
    let error = render_to_wav(&directory, &wav, u32::MAX, Some(16)).unwrap_err();
    assert!(error.to_string().contains("sample rate"), "{error}");
    assert_eq!(std::fs::read(&wav).unwrap(), prior);
    let error = render_to_wav(&directory, &wav, 48_000, Some(u32::MAX)).unwrap_err();
    assert!(error.to_string().contains("32-bit"), "{error}");
    assert_eq!(std::fs::read(&wav).unwrap(), prior);
    assert_eq!(export_lines_for_test(), lines);

    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&fresh);
    drop(_env);
}

fn part_files(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".part"))
        })
        .collect()
}

struct FaultFile(PathBuf);

impl FaultFile {
    fn set(clap_id: &str, fault: &str) -> Self {
        let path = std::env::temp_dir().join(format!("echode-pro07-{clap_id}"));
        std::fs::write(&path, fault).unwrap();
        Self(path)
    }
}

impl Drop for FaultFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn play_plugins(package: &Path, project: &Project) -> Vec<f32> {
    let host = echode_plugin_host::host_binary().unwrap();
    let routes = echode_audio_io::installed_plugin_routes(project);
    let (mut control, mut audio) =
        echode_engine::playback_with_plugins(project, 48_000, PLUGIN_FRAMES, &host, &routes)
            .unwrap();
    let package = package.to_path_buf();
    control.set_clip_loader(Arc::new(move |project, rate| {
        echode_audio_io::load_placed_clips(&package, project, rate)
    }));
    control.replace(project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; PLUGIN_FRAMES];
    let mut right = vec![0.0; PLUGIN_FRAMES];
    // Compare content, not whether a helper met a live callback deadline.
    audio.render_until(
        &mut left,
        &mut right,
        Instant::now() + Duration::from_secs(10),
    );
    assert!(!control.status().failed);
    assert!(control.plugin_failure().is_none());
    left
}

fn wav_lefts(path: &Path, frames: usize) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    (0..frames)
        .map(|frame| {
            let start = 44 + frame * 8;
            f32::from_le_bytes(bytes[start..start + 4].try_into().unwrap())
        })
        .collect()
}

fn write_plugin_cache(cache: &Path, plugin: &Path) {
    let path = json_string(&plugin.display().to_string());
    let line = format!(
        r#"{{"path":{path},"bundle_version":"","modified_secs":0,"modified_nanos":0,"executable_size":0,"plugins":[{{"id":"dev.echode.test-instrument","name":"Echode Test Instrument","vendor":"Echode","version":"0.0.0","features":["instrument"],"url":""}},{{"id":"dev.echode.test-gain","name":"Echode Test Gain","vendor":"Echode","version":"0.0.0","features":["audio-effect"],"url":""}}]}}"#
    );
    std::fs::write(cache, line + "\n").unwrap();
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other if other.is_control() => out.push_str(&format!("\\u{:04x}", other as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn host_bin() -> PathBuf {
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .and_then(|dir| dir.parent())
                .map(|dir| dir.join("echode-plugin-host"))
        })
        .unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = [
        manifest.join("../echode-plugin-host/src"),
        manifest.join("../echode-sandbox/src"),
    ];
    let fresh = path.is_file() && sources.iter().all(|source| newer_than(&path, source));
    if fresh {
        return path;
    }
    let status = std::process::Command::new(env!("CARGO"))
        .current_dir(manifest.join("../.."))
        .args(["build", "-p", "echode-plugin-host"])
        .status()
        .unwrap();
    assert!(status.success());
    path
}

fn newer_than(built: &Path, source: &Path) -> bool {
    let Ok(built) = built.metadata().and_then(|meta| meta.modified()) else {
        return false;
    };
    let Ok(edited) = newest_source(source) else {
        return true;
    };
    built >= edited
}

fn plugin_dylib() -> PathBuf {
    let host = host_bin();
    let sibling = host.parent().unwrap().join("libechode_test_plugin.dylib");
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
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugin");
    let status = std::process::Command::new(env!("CARGO"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(["build", "-p", "echode-test-plugin", "--target-dir"])
        .arg(&target)
        .status()
        .unwrap();
    assert!(status.success());
    target.join("debug").join("libechode_test_plugin.dylib")
}

struct PluginEnv;

impl PluginEnv {
    fn set(cache: &Path) -> Self {
        let host = host_bin();
        // SAFETY: the plugin-export test holds PLUGIN_ENV, and the helper is
        // spawned only after these values are stored.
        unsafe {
            std::env::set_var("ECHODE_PLUGIN_HOST", host);
            std::env::set_var("ECHODE_PLUGIN_CACHE", cache);
        }
        Self
    }

    fn without_host(cache: &Path) -> Self {
        unsafe {
            std::env::set_var("ECHODE_PLUGIN_HOST", cache.with_file_name("no-such-host"));
            std::env::set_var("ECHODE_PLUGIN_CACHE", cache);
        }
        Self
    }
}

impl Drop for PluginEnv {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("ECHODE_PLUGIN_HOST");
            std::env::remove_var("ECHODE_PLUGIN_CACHE");
        }
    }
}

fn clip_gain(project: &Project, clip: Id) -> f32 {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips.iter())
        .find(|placement| placement.id == clip)
        .unwrap()
        .gain
}

/// Rewrites the committed reference. `make bless REASON=...` sets `BLESS=1`.
#[test]
#[ignore = "rewrites tests/fixtures/sine-pad-a4-128.wav; use make bless"]
fn bless_the_reference() {
    let _lock = PLUGIN_ENV.lock().unwrap();
    assert_eq!(
        std::env::var("BLESS").ok().as_deref(),
        Some("1"),
        "set BLESS=1 via make bless REASON=\"...\""
    );
    let directory = scratch();
    let rendered = render_reference(&directory);
    let path = reference_wav();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &rendered).unwrap();
    let _ = std::fs::remove_dir_all(directory);
}

/// Newest edit among the test plugin's sources, so a change to any module rebuilds it.
fn newest_source(dir: &Path) -> std::io::Result<std::time::SystemTime> {
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    for entry in std::fs::read_dir(dir)? {
        newest = newest.max(entry?.metadata()?.modified()?);
    }
    Ok(newest)
}
