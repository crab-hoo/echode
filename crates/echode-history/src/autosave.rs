//! Background autosave of a product package.
//!
//! `save` only enqueues the snapshot. The history thread writes the files and
//! the commit. After the commit succeeds it marks the journal as caught up, so
//! a crash while that mark is still old still recovers the commit itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use echode_model::{Content, Project, Track};
use gix::{ObjectId, Repository};

use crate::document::{
    AUTOSAVE_REF, FilePlan, PackageError, clear_save_pending, commit_tree, mark_save_pending,
    read_commit, write_incremental,
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

    pub fn try_wait(&self) -> Option<Result<ObjectId, PackageError>> {
        match self.reply.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err(PackageError::new("autosave thread stopped")))
            }
        }
    }
}

enum Request {
    Save {
        project: Arc<Project>,
        journal_seq: u64,
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
    contents: HashMap<String, (Arc<Content>, ObjectId)>,
    project_json: String,
    project_blob: ObjectId,
}

impl Autosave {
    pub fn create(directory: &Path) -> Result<Self, PackageError> {
        crate::objects::ensure_store_gitignore(directory)?;
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
    ///
    /// `journal_seq` is the last journal line included in `project`.
    pub fn save(
        &self,
        project: Arc<Project>,
        journal_seq: u64,
    ) -> Result<PendingAutosave, PackageError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Save {
                project,
                journal_seq,
                reply,
            })
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
            Request::Save {
                project,
                journal_seq,
                reply,
            } => {
                // HIS-3: a burst of edits keeps the newest snapshot. Every
                // waiter still hears the commit that includes its edit.
                let coalesced = coalesce_save(project, journal_seq, reply, &incoming);
                let result = save(
                    &mut repo,
                    &directory,
                    &mut cache,
                    &mut parents,
                    &coalesced.project,
                    coalesced.journal_seq,
                );
                answer(coalesced.replies, result);
                match coalesced.next {
                    Some(Request::Shutdown) => break,
                    Some(Request::Load { revision, reply }) => {
                        let _ = reply.send(read_commit(&repo, revision));
                    }
                    Some(Request::Save { .. }) => {
                        unreachable!("coalesce consumes every queued save")
                    }
                    None => {}
                }
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

fn coalesce_save(
    mut project: Arc<Project>,
    mut journal_seq: u64,
    reply: Sender<Result<ObjectId, PackageError>>,
    incoming: &Receiver<Request>,
) -> CoalescedSave {
    let mut replies = vec![reply];
    loop {
        match incoming.try_recv() {
            Ok(Request::Save {
                project: next,
                journal_seq: seq,
                reply,
            }) => {
                replies.push(reply);
                project = next;
                journal_seq = seq;
            }
            Ok(other) => {
                return CoalescedSave {
                    project,
                    journal_seq,
                    replies,
                    next: Some(other),
                };
            }
            Err(_) => {
                return CoalescedSave {
                    project,
                    journal_seq,
                    replies,
                    next: None,
                };
            }
        }
    }
}

struct CoalescedSave {
    project: Arc<Project>,
    journal_seq: u64,
    replies: Vec<Sender<Result<ObjectId, PackageError>>>,
    next: Option<Request>,
}

fn answer(
    replies: Vec<Sender<Result<ObjectId, PackageError>>>,
    result: Result<ObjectId, PackageError>,
) {
    match result {
        Ok(id) => {
            for reply in replies {
                let _ = reply.send(Ok(id));
            }
        }
        Err(error) => {
            let message = error.to_string();
            for reply in replies {
                let _ = reply.send(Err(PackageError::new(message.clone())));
            }
        }
    }
}

fn save(
    repo: &mut Repository,
    directory: &Path,
    cache: &mut Option<Cache>,
    parents: &mut Vec<ObjectId>,
    project: &Project,
    journal_seq: u64,
) -> Result<ObjectId, PackageError> {
    mark_save_pending(directory)?;
    let plan = plan_files(cache, project);
    let written = {
        let file_plan = FilePlan {
            unchanged_tracks: &plan.unchanged_tracks,
            unchanged_contents: &plan.unchanged_contents,
            previous_project_json: cache.as_ref().map(|cache| cache.project_json.as_str()),
            removed_tracks: &plan.removed_tracks,
            removed_contents: &plan.removed_contents,
            sweep: plan.sweep,
        };
        write_incremental(directory, project, &file_plan)?
    };
    let project_blob = match cache
        .as_ref()
        .filter(|cache| cache.project_json == written.project_json)
    {
        Some(cache) => cache.project_blob,
        None => repo
            .write_blob(written.project_json.as_bytes())
            .map_err(|error| PackageError::new(error.to_string()))?
            .detach(),
    };
    let changed_contents: HashMap<String, String> = written.contents.into_iter().collect();
    let changed_tracks: HashMap<String, String> = written.tracks.into_iter().collect();
    let mut content_blobs = Vec::with_capacity(project.contents().len());
    let mut stored_contents = HashMap::with_capacity(project.contents().len());
    for content in project.contents() {
        let id = content.id.to_string();
        let filename = format!("{id}.json");
        if plan.unchanged_contents.contains(&id) {
            let (cached, blob) = cache
                .as_ref()
                .and_then(|cache| cache.contents.get(&id))
                .ok_or_else(|| PackageError::new("unchanged content is missing from the cache"))?;
            content_blobs.push((filename, *blob));
            stored_contents.insert(id, (Arc::clone(cached), *blob));
            continue;
        }
        let json = changed_contents
            .get(&filename)
            .ok_or_else(|| PackageError::new("a changed content was not written"))?;
        let blob = repo
            .write_blob(json.as_bytes())
            .map_err(|error| PackageError::new(error.to_string()))?
            .detach();
        content_blobs.push((filename, blob));
        stored_contents.insert(id, (Arc::clone(content), blob));
    }
    let mut track_blobs = Vec::with_capacity(project.tracks().len());
    let mut stored = HashMap::with_capacity(project.tracks().len());
    for track in project.tracks() {
        let id = track.id.to_string();
        let filename = format!("{id}.json");
        // HIS-1: hold the Arc. An address alone can be reused after the
        // previous snapshot is freed, and then a changed track looks unchanged.
        if plan.unchanged_tracks.contains(&id) {
            let (cached, blob) = cache
                .as_ref()
                .and_then(|cache| cache.tracks.get(&id))
                .ok_or_else(|| PackageError::new("unchanged track is missing from the cache"))?;
            track_blobs.push((filename, *blob));
            stored.insert(id, (Arc::clone(cached), *blob));
            continue;
        }
        let json = changed_tracks
            .get(&filename)
            .ok_or_else(|| PackageError::new("a changed track was not written"))?;
        let blob = repo
            .write_blob(json.as_bytes())
            .map_err(|error| PackageError::new(error.to_string()))?
            .detach();
        track_blobs.push((filename, blob));
        stored.insert(id, (Arc::clone(track), blob));
    }
    let id = commit_tree(
        repo,
        project_blob,
        &content_blobs,
        &track_blobs,
        parents,
        journal_seq,
    )?;
    *parents = vec![id];
    clear_save_pending(directory)?;
    *cache = Some(Cache {
        tracks: stored,
        contents: stored_contents,
        project_json: written.project_json,
        project_blob,
    });
    Ok(id)
}

struct SavePlan {
    unchanged_tracks: HashSet<String>,
    unchanged_contents: HashSet<String>,
    removed_tracks: Vec<String>,
    removed_contents: Vec<String>,
    sweep: bool,
}

fn plan_files(cache: &Option<Cache>, project: &Project) -> SavePlan {
    let Some(cache) = cache else {
        return SavePlan {
            unchanged_tracks: HashSet::new(),
            unchanged_contents: HashSet::new(),
            removed_tracks: Vec::new(),
            removed_contents: Vec::new(),
            sweep: true,
        };
    };
    let mut unchanged_tracks = HashSet::new();
    let mut live_tracks = HashSet::new();
    for track in project.tracks() {
        let id = track.id.to_string();
        if cache
            .tracks
            .get(&id)
            .is_some_and(|(cached, _)| Arc::ptr_eq(cached, track))
        {
            unchanged_tracks.insert(id.clone());
        }
        live_tracks.insert(id);
    }
    let mut unchanged_contents = HashSet::new();
    let mut live_contents = HashSet::new();
    for content in project.contents() {
        let id = content.id.to_string();
        if cache
            .contents
            .get(&id)
            .is_some_and(|(cached, _)| Arc::ptr_eq(cached, content))
        {
            unchanged_contents.insert(id.clone());
        }
        live_contents.insert(id);
    }
    SavePlan {
        removed_tracks: cache
            .tracks
            .keys()
            .filter(|id| !live_tracks.contains(id.as_str()))
            .cloned()
            .collect(),
        removed_contents: cache
            .contents
            .keys()
            .filter(|id| !live_contents.contains(id.as_str()))
            .cloned()
            .collect(),
        unchanged_tracks,
        unchanged_contents,
        sweep: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_saves_collapse_to_the_latest() {
        let (sender, incoming) = mpsc::channel();
        let mut project = Project::new();
        let mut first = None;
        for seq in 1..=3 {
            let (next, _) = project.add_track(format!("t{seq}"));
            project = next;
            let (reply, _) = mpsc::channel();
            let request = Request::Save {
                project: Arc::new(project.clone()),
                journal_seq: seq,
                reply,
            };
            if first.is_none() {
                first = Some(request);
            } else {
                sender.send(request).unwrap();
            }
        }
        sender.send(Request::Shutdown).unwrap();
        let Request::Save {
            project,
            journal_seq,
            reply,
        } = first.unwrap()
        else {
            unreachable!("the first request is a save");
        };
        let coalesced = coalesce_save(project, journal_seq, reply, &incoming);
        assert_eq!(coalesced.journal_seq, 3);
        assert_eq!(coalesced.project.tracks().len(), 3);
        assert_eq!(coalesced.replies.len(), 3);
        assert!(matches!(coalesced.next, Some(Request::Shutdown)));
    }
}
