//! Background autosave of a product package.
//!
//! `save` only enqueues the snapshot. The history thread writes the files and
//! the commit. After the commit succeeds it marks the journal as caught up, so
//! a crash while that mark is still old still recovers the commit itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use echode_model::{Project, Track};
use gix::{ObjectId, Repository};

use crate::document::{
    AUTOSAVE_REF, JournalBase, PackageError, commit_tree, read_commit, render, write_files,
    write_journal,
};

pub struct Autosave {
    request: Sender<Request>,
    thread: Option<JoinHandle<()>>,
}

pub struct PendingAutosave {
    reply: Receiver<Result<ObjectId, PackageError>>,
}

impl PendingAutosave {
    pub fn wait(self) -> Result<ObjectId, PackageError> {
        self.reply
            .recv()
            .map_err(|_| PackageError::new("autosave thread stopped"))?
    }
}

enum Request {
    Save {
        project: Arc<Project>,
        reply: Sender<Result<ObjectId, PackageError>>,
    },
    Load {
        revision: ObjectId,
        reply: Sender<Result<Project, PackageError>>,
    },
    Shutdown,
}

struct Cache {
    tracks: HashMap<String, (Arc<Track>, ObjectId)>,
    project_json: String,
    project_blob: ObjectId,
}

impl Autosave {
    pub fn create(directory: &Path) -> Result<Self, PackageError> {
        let directory = directory.to_path_buf();
        let (request, incoming) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("echode-autosave".to_string())
            .spawn(move || worker(directory, incoming))
            .map_err(|error| PackageError::new(error.to_string()))?;
        Ok(Self {
            request,
            thread: Some(thread),
        })
    }

    /// Hand the snapshot to the autosave thread and return.
    pub fn save(&self, project: Arc<Project>) -> Result<PendingAutosave, PackageError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Save { project, reply })
            .map_err(|_| PackageError::new("autosave thread stopped"))?;
        Ok(PendingAutosave { reply: incoming })
    }

    pub fn load(&self, revision: ObjectId) -> Result<Project, PackageError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Load { revision, reply })
            .map_err(|_| PackageError::new("autosave thread stopped"))?;
        incoming
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| PackageError::new("autosave thread stopped"))?
    }
}

impl Drop for Autosave {
    fn drop(&mut self) {
        let _ = self.request.send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn open_or_init(directory: &Path) -> Result<Repository, String> {
    match gix::open(directory) {
        Ok(repo) => Ok(repo),
        Err(_) => gix::init(directory).map_err(|error| error.to_string()),
    }
}

fn tip(repo: &Repository) -> Option<ObjectId> {
    let reference = repo.try_find_reference(AUTOSAVE_REF).ok()??;
    Some(reference.id().detach())
}

fn worker(directory: PathBuf, incoming: Receiver<Request>) {
    let mut repo = match open_or_init(&directory) {
        Ok(repo) => repo,
        Err(error) => {
            while let Ok(request) = incoming.recv() {
                fail(request, error.clone());
            }
            return;
        }
    };
    let mut parents = Vec::new();
    if let Some(id) = tip(&repo) {
        parents.push(id);
    }
    let mut cache = None;
    while let Ok(request) = incoming.recv() {
        match request {
            Request::Shutdown => break,
            Request::Save { project, reply } => {
                let result = save(&mut repo, &directory, &mut cache, &mut parents, &project);
                let _ = reply.send(result);
            }
            Request::Load { revision, reply } => {
                let result = read_commit(&repo, revision);
                let _ = reply.send(result);
            }
        }
    }
}

fn fail(request: Request, message: String) {
    let error = PackageError::new(message);
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
) -> Result<ObjectId, PackageError> {
    let package = render(project);
    write_files(directory, &package)?;
    let project_blob = match cache
        .as_ref()
        .filter(|cache| cache.project_json == package.project_json)
    {
        Some(cache) => cache.project_blob,
        None => repo
            .write_blob(package.project_json.as_bytes())
            .map_err(|error| PackageError::new(error.to_string()))?
            .detach(),
    };
    let mut track_blobs = Vec::with_capacity(package.tracks.len());
    let mut stored = HashMap::with_capacity(package.tracks.len());
    for (track, (filename, json)) in project.tracks().iter().zip(&package.tracks) {
        let id = track.id.to_string();
        // HIS-1: hold the Arc. An address alone can be reused after the
        // previous snapshot is freed, and then a changed track looks unchanged.
        if let Some((cached, blob)) = cache
            .as_ref()
            .and_then(|cache| cache.tracks.get(&id))
            .filter(|(cached, _)| Arc::ptr_eq(cached, track))
        {
            track_blobs.push((filename.clone(), *blob));
            stored.insert(id, (Arc::clone(cached), *blob));
            continue;
        }
        let blob = repo
            .write_blob(json.as_bytes())
            .map_err(|error| PackageError::new(error.to_string()))?
            .detach();
        track_blobs.push((filename.clone(), blob));
        stored.insert(id, (Arc::clone(track), blob));
    }
    let id = commit_tree(repo, project_blob, &track_blobs, parents)?;
    *parents = vec![id];
    write_journal(directory, JournalBase::Commit(id), &[])?;
    *cache = Some(Cache {
        tracks: stored,
        project_json: package.project_json,
        project_blob,
    });
    Ok(id)
}
