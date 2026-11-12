//! Release measurement of what an import costs the document thread, run by
//! `make spike-import` (ADR 0331). Never part of the normal checks.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use echode_actions::Command;
use echode_history::{DocumentHost, StagedImports, cache_peaks, write_package};
use echode_model::{Id, Project, Tick};

/// The measured file: three minutes of 16-bit stereo at 44.1 kHz, a common
/// song length. A fixture of this measurement, not a product limit.
const SECONDS: u32 = 180;
const RATE: u32 = 44_100;
const RUNS: usize = 5;

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("echode-import-timing-{name}-{}", Id::generate()))
}

fn write_wav(path: &Path) {
    let frames = SECONDS * RATE;
    let data = frames * 4;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for frame in 0..frames {
        // A tone that differs per run would only change the hash; this one
        // keeps every sample nonzero so nothing compresses away.
        let left = ((frame % 200) as i16 - 100) * 100;
        bytes.extend_from_slice(&left.to_le_bytes());
        bytes.extend_from_slice(&(-left).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A fresh package with one track, so no import finds its object stored.
fn package() -> (PathBuf, DocumentHost, Id) {
    let path = scratch("package");
    write_package(&path, &Project::new()).unwrap();
    let host = DocumentHost::open(&path).unwrap();
    host.perform(Command::AddTrack {
        name: "import".into(),
    })
    .unwrap();
    let track = host.snapshot().tracks()[0].id;
    (path, host, track)
}

fn import(track: Id, source: &str) -> Command {
    Command::ImportAudio {
        track,
        start: Tick::new(0),
        path: source.to_owned(),
    }
}

fn summary(label: &str, mut times: Vec<Duration>) {
    times.sort();
    let ms = |time: Duration| time.as_secs_f64() * 1_000.0;
    println!(
        "{label}: min {:.1} ms, median {:.1} ms, max {:.1} ms ({} runs)",
        ms(times[0]),
        ms(times[times.len() / 2]),
        ms(times[times.len() - 1]),
        times.len()
    );
}

#[test]
#[ignore = "release timing; run with make spike-import"]
fn measure_import_on_and_off_the_document_thread() {
    let wav = scratch("source").with_extension("wav");
    write_wav(&wav);
    let source = wav.to_string_lossy().into_owned();
    // Warm the page cache, as a file just dragged from the browser would be.
    let _ = std::fs::read(&wav).unwrap();

    let mut on_thread = Vec::new();
    let mut read_back = Vec::new();
    let mut staging = Vec::new();
    let mut landing = Vec::new();
    for _ in 0..RUNS {
        let (path, host, track) = package();
        let started = Instant::now();
        let action = host.perform(import(track, &source)).unwrap();
        on_thread.push(started.elapsed());
        let hash = match action {
            echode_actions::Action::PlaceAudio { hash, .. } => hash,
            other => panic!("an import places audio: {other:?}"),
        };
        // What publishing used to add: reading the object back for peaks.
        let started = Instant::now();
        cache_peaks(&path, &hash).unwrap();
        read_back.push(started.elapsed());
        drop(host);
        let _ = std::fs::remove_dir_all(&path);

        let (path, host, track) = package();
        let started = Instant::now();
        let staged = StagedImports::stage(&path, std::slice::from_ref(&source), &|| true);
        staging.push(started.elapsed());
        let started = Instant::now();
        host.perform_staged(import(track, &source), staged).unwrap();
        landing.push(started.elapsed());
        drop(host);
        let _ = std::fs::remove_dir_all(&path);
    }
    let _ = std::fs::remove_file(&wav);
    println!("{SECONDS} s of 16-bit stereo at {RATE} Hz");
    summary("staged on the document thread", on_thread);
    summary("peaks read back from the object (removed)", read_back);
    summary("staged off the document thread", staging);
    summary("document thread after staging", landing);
}
