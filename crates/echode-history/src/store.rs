//! Background commits through gix.
//!
//! `gix` is MIT OR Apache-2.0, which can be combined with Echode's
//! GPL-3.0-or-later. The caller of [`History::save`] only enqueues an
//! `Arc` snapshot. Serialization and the commit happen on a worker thread.

use crate::format::{self, parse_project_tracks, parse_track};
use crate::project::Project;
use gix::bstr::BString;
use gix::objs::Tree;
use gix::objs::tree::{Entry, EntryKind};
use gix::{ObjectId, Repository};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct History {
    request: Sender<Request>,
    thread: Option<JoinHandle<()>>,
}

pub struct PendingSave {
    reply: Receiver<Result<ObjectId, String>>,
}

impl PendingSave {
    pub fn wait(self) -> Result<ObjectId, String> {
        self.reply
            .recv()
            .map_err(|_| "history thread stopped".to_string())?
    }
}

enum Request {
    Save {
        project: Arc<Project>,
        reply: Sender<Result<ObjectId, String>>,
    },
    Load {
        revision: ObjectId,
        reply: Sender<Result<Arc<Project>, String>>,
    },
    Shutdown,
}

struct Cache {
    tracks: Vec<Arc<crate::project::Track>>,
    blobs: Vec<ObjectId>,
    project_json: String,
    project_blob: ObjectId,
}

impl History {
    pub fn create(directory: &Path) -> Result<Self, String> {
        let directory = directory.to_path_buf();
        let (request, incoming) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("echode-history".to_string())
            .spawn(move || worker(directory, incoming))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            request,
            thread: Some(thread),
        })
    }

    /// Hand the snapshot to the history thread and return.
    pub fn save(&self, project: Arc<Project>) -> Result<PendingSave, String> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Save { project, reply })
            .map_err(|_| "history thread stopped".to_string())?;
        Ok(PendingSave { reply: incoming })
    }

    pub fn load(&self, revision: ObjectId) -> Result<Arc<Project>, String> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Load { revision, reply })
            .map_err(|_| "history thread stopped".to_string())?;
        incoming
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| "history thread stopped".to_string())?
    }
}

impl Drop for History {
    fn drop(&mut self) {
        let _ = self.request.send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn worker(directory: std::path::PathBuf, incoming: Receiver<Request>) {
    let mut repo = match gix::init(&directory) {
        Ok(repo) => repo,
        Err(error) => {
            while let Ok(request) = incoming.recv() {
                fail(request, error.to_string());
            }
            return;
        }
    };
    let mut cache: Option<Cache> = None;
    let mut parents: Vec<ObjectId> = Vec::new();
    while let Ok(request) = incoming.recv() {
        match request {
            Request::Shutdown => break,
            Request::Save { project, reply } => {
                let result = save(&mut repo, &directory, &mut cache, &mut parents, &project);
                let _ = reply.send(result);
            }
            Request::Load { revision, reply } => {
                let result = load(&repo, revision);
                let _ = reply.send(result);
            }
        }
    }
}

fn fail(request: Request, error: String) {
    match request {
        Request::Save { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Request::Load { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Request::Shutdown => {}
    }
}

fn save(
    repo: &mut Repository,
    directory: &Path,
    cache: &mut Option<Cache>,
    parents: &mut Vec<ObjectId>,
    project: &Project,
) -> Result<ObjectId, String> {
    std::fs::create_dir_all(directory.join("tracks")).map_err(|error| error.to_string())?;
    let mut blobs = Vec::with_capacity(project.tracks.len());
    for (index, track) in project.tracks.iter().enumerate() {
        let unchanged = cache.as_ref().is_some_and(|cache| {
            cache
                .tracks
                .get(index)
                .is_some_and(|previous| Arc::ptr_eq(previous, track))
        });
        if unchanged {
            blobs.push(cache.as_ref().unwrap().blobs[index]);
            continue;
        }
        let json = format::track_json(track);
        let path = directory.join(format!("tracks/{}.json", track.id));
        std::fs::write(&path, &json).map_err(|error| error.to_string())?;
        let blob = repo
            .write_blob(json.as_bytes())
            .map_err(|error| error.to_string())?;
        blobs.push(blob.detach());
    }

    let project_json = format::project_json(project);
    let project_blob = if cache
        .as_ref()
        .is_some_and(|cache| cache.project_json == project_json)
    {
        cache.as_ref().unwrap().project_blob
    } else {
        std::fs::write(directory.join("project.json"), &project_json)
            .map_err(|error| error.to_string())?;
        repo.write_blob(project_json.as_bytes())
            .map_err(|error| error.to_string())?
            .detach()
    };

    let mut track_entries = Vec::with_capacity(blobs.len());
    for (track, blob) in project.tracks.iter().zip(&blobs) {
        track_entries.push(Entry {
            mode: EntryKind::Blob.into(),
            filename: BString::from(format!("{}.json", track.id)),
            oid: *blob,
        });
    }
    track_entries.sort();
    let tracks_tree = repo
        .write_object(Tree {
            entries: track_entries,
        })
        .map_err(|error| error.to_string())?
        .detach();

    let mut root_entries = vec![
        Entry {
            mode: EntryKind::Blob.into(),
            filename: BString::from("project.json"),
            oid: project_blob,
        },
        Entry {
            mode: EntryKind::Tree.into(),
            filename: BString::from("tracks"),
            oid: tracks_tree,
        },
    ];
    root_entries.sort();
    let root = repo
        .write_object(Tree {
            entries: root_entries,
        })
        .map_err(|error| error.to_string())?
        .detach();

    let author = gix::actor::Signature {
        name: "Echode".into(),
        email: "history@echode.local".into(),
        time: gix::date::Time::now_local_or_utc(),
    };
    let mut author_time = gix::date::parse::TimeBuf::default();
    let mut committer_time = gix::date::parse::TimeBuf::default();
    let commit = repo
        .commit_as(
            author.to_ref(&mut committer_time),
            author.to_ref(&mut author_time),
            "refs/heads/main",
            "autosave",
            root,
            parents.iter().copied(),
        )
        .map_err(|error| error.to_string())?;
    let id = commit.detach();
    parents.clear();
    parents.push(id);
    *cache = Some(Cache {
        tracks: project.tracks.clone(),
        blobs,
        project_json,
        project_blob,
    });
    Ok(id)
}

fn load(repo: &Repository, revision: ObjectId) -> Result<Arc<Project>, String> {
    let commit = repo
        .find_commit(revision)
        .map_err(|error| error.to_string())?;
    let root = commit.tree().map_err(|error| error.to_string())?;
    let project_entry = root
        .lookup_entry_by_path("project.json")
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "project.json is missing from the revision".to_string())?;
    let project_blob = project_entry
        .object()
        .map_err(|error| error.to_string())?
        .try_into_blob()
        .map_err(|error| format!("{error:?}"))?;
    let project_json =
        std::str::from_utf8(&project_blob.data).map_err(|error| error.to_string())?;
    let ids = parse_project_tracks(project_json)?;

    let tracks_entry = root
        .lookup_entry_by_path("tracks")
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "tracks directory is missing from the revision".to_string())?;
    let tracks_tree = tracks_entry
        .object()
        .map_err(|error| error.to_string())?
        .try_into_tree()
        .map_err(|error| format!("{error:?}"))?;
    let mut by_name = std::collections::HashMap::with_capacity(ids.len());
    for entry in tracks_tree.iter() {
        let entry = entry.map_err(|error| error.to_string())?;
        let filename = entry.filename().to_string();
        let blob = entry
            .object()
            .map_err(|error| error.to_string())?
            .try_into_blob()
            .map_err(|error| format!("{error:?}"))?;
        let json = std::str::from_utf8(&blob.data).map_err(|error| error.to_string())?;
        by_name.insert(filename, parse_track(json)?);
    }
    let mut tracks = Vec::with_capacity(ids.len());
    for id in ids {
        let track = by_name
            .remove(&format!("{id}.json"))
            .ok_or_else(|| format!("tracks/{id}.json is missing"))?;
        tracks.push(Arc::new(track));
    }
    Ok(Arc::new(Project { tracks }))
}
