//! Replacement media decodes away from the document's control turn (ADR 0147).
use super::{ClipLoader, LoadedClip, PlaybackError};
use echode_model::{AudioRef, Id, Placement, Project, TempoMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread::JoinHandle;

/// What a clip loader reads from a project at one device rate: the tempo map
/// and every audio placement with the audio it refers to.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaInputs {
    tempo: TempoMap,
    placements: Vec<(Id, Placement, AudioRef)>,
}

impl MediaInputs {
    pub fn of(project: &Project) -> Self {
        let placements = project
            .tracks()
            .iter()
            .flat_map(|track| {
                track.clips.iter().filter_map(|clip| {
                    let audio = project.content(clip.content)?.audio.as_ref()?;
                    Some((track.id, clip.clone(), audio.clone()))
                })
            })
            .collect();
        Self {
            tempo: project.tempo().clone(),
            placements,
        }
    }
}

pub(super) struct Loaded {
    pub(super) inputs: MediaInputs,
    /// The control's replacement count when the load began.
    pub(super) replacement: u64,
    pub(super) clips: Result<Vec<LoadedClip>, PlaybackError>,
}

/// Dropping an unfinished load detaches it. It reads package objects only and
/// starts no helper, so a closed output need not wait for its files.
pub(super) struct MediaPreparation {
    thread: Option<JoinHandle<Loaded>>,
}

impl MediaPreparation {
    pub(super) fn begin(
        loader: ClipLoader,
        project: Project,
        inputs: MediaInputs,
        rate: u32,
        replacement: u64,
    ) -> Result<Self, PlaybackError> {
        let thread = std::thread::Builder::new()
            .name("echode-media-prepare".into())
            .spawn(move || {
                let clips = catch_unwind(AssertUnwindSafe(|| loader(&project, rate)))
                    .unwrap_or(Err(PlaybackError::Media));
                Loaded {
                    inputs,
                    replacement,
                    clips,
                }
            })
            .map_err(|error| PlaybackError::Unsupported(error.to_string()))?;
        Ok(Self {
            thread: Some(thread),
        })
    }

    /// Joins only an exited thread, so the control turn never waits on a file.
    pub(super) fn poll(&mut self) -> Option<Result<Loaded, PlaybackError>> {
        if !self.thread.as_ref()?.is_finished() {
            return None;
        }
        Some(self.thread.take()?.join().map_err(|_| PlaybackError::Media))
    }
}
