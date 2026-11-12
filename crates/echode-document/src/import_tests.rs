//! An import staging off the worker, observed through the service with a
//! device-free output and the product's staging behind a gate (ADR 0331).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use echode_audio_io::HeadlessOutput;
use echode_history::write_package;
use echode_model::{Id, Project};

use super::*;
use crate::{DocumentService, Presenter, Publication, Ticket};

const WAIT: Duration = Duration::from_secs(5);
/// How long a held job is watched for finishing too early. Not a deadline.
const HELD: Duration = Duration::from_millis(200);

struct View {
    tracks: usize,
    first: Option<Id>,
    clips: usize,
    audio: serde_json::Value,
}

struct Counts;

impl Presenter for Counts {
    type View = View;

    fn present(&mut self, publication: Publication<'_>) -> View {
        let tracks = publication.project.tracks();
        View {
            tracks: tracks.len(),
            first: tracks.first().map(|track| track.id),
            clips: tracks.iter().map(|track| track.clips.len()).sum(),
            audio: serde_json::from_slice(&publication.audio).unwrap(),
        }
    }

    fn with_device_report(view: View, _report: Vec<u8>) -> View {
        view
    }
}

/// The product's staging, held until released.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<(bool, u32)>, Condvar)>);

impl Gate {
    /// A failed assertion elsewhere must not stop a release.
    fn state(&self) -> MutexGuard<'_, (bool, u32)> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn hold(&self) {
        self.state().0 = true;
    }

    fn release(&self) {
        self.state().0 = false;
        self.0.1.notify_all();
    }

    /// Waits until `count` stagings have reached the gate.
    fn entered(&self, count: u32) {
        let (state, timeout) = self
            .0
            .1
            .wait_timeout_while(self.state(), WAIT, |state| state.1 < count)
            .unwrap_or_else(PoisonError::into_inner);
        drop(state);
        assert!(
            !timeout.timed_out(),
            "the import never staged off the worker"
        );
    }

    fn stage(&self) -> Stage {
        let gate = self.clone();
        Arc::new(
            move |package: &Path, sources: &[String], keep_going: &dyn Fn() -> bool| {
                let changed = &gate.0.1;
                let mut held = gate.state();
                held.1 += 1;
                changed.notify_all();
                while held.0 {
                    held = changed.wait(held).unwrap_or_else(PoisonError::into_inner);
                }
                drop(held);
                StagedImports::stage(package, sources, keep_going)
            },
        )
    }
}

/// Releases the gate when a failing test unwinds, so closing the document
/// does not wait forever for the import it holds.
struct Release(Gate);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("echode-import-{name}-{}", Id::generate()))
}

/// One second of mono float WAV at 8 kHz.
fn write_wav(path: &Path) {
    const RATE: u32 = 8_000;
    let data = RATE * 4;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for _ in 0..RATE {
        bytes.extend_from_slice(&0.25f32.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn open(path: &Path, gate: &Gate) -> DocumentService<View> {
    DocumentService::open_staging(
        path,
        Counts,
        Box::new(|session| session.use_headless_output(HeadlessOutput::default())),
        gate.stage(),
    )
    .unwrap()
}

fn submit(
    service: &DocumentService<View>,
    operation: Operation,
) -> Receiver<Result<View, DocumentError>> {
    let (sender, done) = channel();
    service.submit(
        operation,
        Ticket::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    done
}

fn finish(service: &DocumentService<View>, operation: Operation) -> View {
    submit(service, operation)
        .recv_timeout(WAIT)
        .unwrap()
        .unwrap()
}

fn edit(json: &str) -> Operation {
    Operation::Perform(json.as_bytes().to_vec())
}

fn import(track: Id, wav: &Path) -> Operation {
    edit(&format!(
        r#"{{"type":"audio.import","track":"{track}","start":0,"path":{}}}"#,
        serde_json::to_string(&wav.to_string_lossy()).unwrap()
    ))
}

/// Nothing arrives within [`HELD`]: the job is still waiting.
fn still_held(pending: &Receiver<Result<View, DocumentError>>, what: &str) {
    assert!(
        pending.recv_timeout(HELD).is_err(),
        "{what} finished while the import staged"
    );
}

#[test]
fn contract_an_import_stages_off_the_worker_and_later_edits_follow_it() {
    let path = scratch("package");
    let wav = scratch("source").with_extension("wav");
    write_package(&path, &Project::new()).unwrap();
    write_wav(&wav);
    let gate = Gate::default();
    let service = open(&path, &gate);
    let _release = Release(gate.clone());
    let track = finish(&service, edit(r#"{"type":"track.add","name":"drop"}"#))
        .first
        .unwrap();

    gate.hold();
    let imported = submit(&service, import(track, &wav));
    gate.entered(1);
    // Transport, snapshots and device requests take their turns meanwhile,
    // and the worker keeps publishing the playhead.
    let located = finish(
        &service,
        Operation::Audio(br#"{"type":"transport.locate","tick":960}"#.to_vec()),
    );
    assert_eq!(located.audio["tick"], 960);
    assert_eq!(service.position().0, 960);
    let snapshot = finish(&service, Operation::Snapshot);
    assert_eq!((snapshot.tracks, snapshot.clips), (1, 0));
    let close = format!(
        r#"{{"type":"device.gui.close","device":"{}"}}"#,
        Id::from_raw(0xD0)
    );
    finish(&service, Operation::Device(close.into_bytes()));
    // Edits, history and Undo wait behind the import, in submission order.
    let added = submit(&service, edit(r#"{"type":"track.add","name":"later"}"#));
    let undone = submit(&service, Operation::Undo);
    let undone_again = submit(&service, Operation::Undo);
    still_held(&imported, "the import");
    still_held(&added, "an edit submitted after the import");
    still_held(&undone, "an Undo submitted after the import");

    gate.release();
    let landed = imported.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!((landed.tracks, landed.clips), (1, 1));
    let added = added.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!((added.tracks, added.clips), (2, 1));
    let undone = undone.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(
        (undone.tracks, undone.clips),
        (1, 1),
        "Undo took the later edit"
    );
    let undone = undone_again.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!((undone.tracks, undone.clips), (1, 0), "then the import");

    // Closing finishes an import still staging: every submission completes.
    gate.hold();
    let imported = submit(&service, import(track, &wav));
    gate.entered(2);
    let closing = std::thread::spawn(move || drop(service));
    still_held(&imported, "the import of a closing document");
    gate.release();
    let landed = imported.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(landed.clips, 1);
    let started = Instant::now();
    closing.join().unwrap();
    assert!(started.elapsed() < WAIT);
    let reopened = echode_history::DocumentHost::open(&path).unwrap();
    let clips: usize = reopened
        .snapshot()
        .tracks()
        .iter()
        .map(|track| track.clips.len())
        .sum();
    drop(reopened);
    let _ = std::fs::remove_dir_all(&path);
    let _ = std::fs::remove_file(&wav);
    assert_eq!(clips, 1, "the import that landed while closing was saved");
}

fn panicking_stage(_: &Path, _: &[String], _: &dyn Fn() -> bool) -> StagedImports {
    panic!("staging fails")
}

/// A staging thread that dies fails its edit alone. The document keeps
/// taking edits, and the jobs held behind it run.
#[test]
fn a_staging_thread_that_panics_fails_only_its_edit() {
    let path = scratch("package");
    let wav = scratch("source").with_extension("wav");
    write_package(&path, &Project::new()).unwrap();
    write_wav(&wav);
    let service = DocumentService::open_staging(
        &path,
        Counts,
        Box::new(|session| session.use_headless_output(HeadlessOutput::default())),
        Arc::new(panicking_stage),
    )
    .unwrap();
    let track = finish(&service, edit(r#"{"type":"track.add","name":"drop"}"#))
        .first
        .unwrap();
    let imported = submit(&service, import(track, &wav));
    let added = submit(&service, edit(r#"{"type":"track.add","name":"later"}"#));
    assert!(matches!(
        imported.recv_timeout(WAIT).unwrap(),
        Err(DocumentError::ImportPanicked)
    ));
    let added = added.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!((added.tracks, added.clips), (2, 0));
    drop(service);
    let objects = std::fs::read_dir(path.join("media").join("objects"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    let _ = std::fs::remove_dir_all(&path);
    let _ = std::fs::remove_file(&wav);
    assert_eq!(objects, 0, "no staged bytes remain");
}
