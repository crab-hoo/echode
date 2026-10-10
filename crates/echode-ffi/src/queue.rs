//! A submission returns before the document thread finishes the command.

use std::ffi::{CString, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use echode_actions::Command;
use echode_history::DocumentHost;
use echode_model::Project;

use crate::{Complete, ECHODE_ERR, ECHODE_OK};

/// An open package. The worker, not the caller, runs [`DocumentHost::perform`].
pub struct Document {
    queue: Mutex<Option<Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// One owned reference to a project snapshot.
pub struct Snapshot {
    project: Arc<Project>,
}

struct Context(*mut c_void);

// The pointer is the caller's until the completion returns it. This thread
// does not dereference it.
unsafe impl Send for Context {}

struct Job {
    bytes: Vec<u8>,
    context: Context,
    complete: Complete,
}

impl Document {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let host = DocumentHost::open(path).map_err(|error| error.to_string())?;
        let (sender, incoming) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("echode-ffi".to_string())
            .spawn(move || worker(host, incoming))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            queue: Mutex::new(Some(sender)),
            thread: Mutex::new(Some(thread)),
        })
    }

    pub(crate) fn shutdown(&self) {
        {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            queue.take();
        }
        let mut thread = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(thread) = thread.take() {
            let _ = thread.join();
        }
    }

    pub(crate) fn submit(&self, bytes: &[u8], context: *mut c_void, complete: Complete) {
        let job = Job {
            bytes: bytes.to_vec(),
            context: Context(context),
            complete,
        };
        let rejected = {
            let queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match queue.as_ref() {
                None => Some(job),
                Some(sender) => sender.send(job).err().map(|error| error.0),
            }
        };
        if let Some(job) = rejected {
            finish(&job, Err("document thread stopped".to_string()));
        }
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Snapshot {
    pub(crate) fn retain(&self) -> Self {
        Self {
            project: Arc::clone(&self.project),
        }
    }

    pub(crate) fn track_count(&self) -> u32 {
        u32::try_from(self.project.tracks().len()).unwrap_or(u32::MAX)
    }
}

fn worker(host: DocumentHost, incoming: Receiver<Job>) {
    let mut accepting = true;
    while let Ok(job) = incoming.recv() {
        if !accepting {
            finish(&job, Err("document thread stopped".to_string()));
            continue;
        }
        let text = match str::from_utf8(&job.bytes) {
            Ok(text) if !text.is_empty() => text,
            Ok(_) => {
                finish(&job, Err("command is empty".to_string()));
                continue;
            }
            Err(_) => {
                finish(&job, Err("command is not utf-8".to_string()));
                continue;
            }
        };
        let command = match serde_json::from_str::<Command>(text) {
            Ok(command) => command,
            Err(error) => {
                finish(&job, Err(error.to_string()));
                continue;
            }
        };
        let performed = catch_unwind(AssertUnwindSafe(|| host.perform(command)));
        match performed {
            Ok(Ok(_)) => {
                let snapshot = Box::into_raw(Box::new(Snapshot {
                    project: host.snapshot(),
                }));
                finish_raw(&job, ECHODE_OK, None, snapshot);
            }
            Ok(Err(error)) => finish(&job, Err(error.to_string())),
            Err(_) => {
                accepting = false;
                finish(&job, Err("document thread panicked".to_string()));
            }
        }
    }
}

fn finish(job: &Job, result: Result<(), String>) {
    match result {
        Ok(()) => finish_raw(job, ECHODE_OK, None, std::ptr::null_mut()),
        Err(message) => finish_raw(job, ECHODE_ERR, Some(message), std::ptr::null_mut()),
    }
}

fn finish_raw(job: &Job, status: i32, message: Option<String>, snapshot: *mut Snapshot) {
    let c_message = message.and_then(|message| CString::new(message).ok());
    let pointer = c_message
        .as_ref()
        .map_or(std::ptr::null(), |message| message.as_ptr());
    (job.complete)(job.context.0, status, pointer, snapshot);
}
