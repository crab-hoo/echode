//! Temporary, depth-independent validation of track parent links.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Id, Track};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ForestError {
    MissingParent { track: Id, parent: Id },
    Cycle { track: Id },
}

#[derive(Clone, Copy)]
enum Visit {
    Unvisited,
    Active,
    Resolved,
}

/// Visits each track once, even when many paths share the same ancestors.
/// Neither the traversal nor its work bound relies on arrange order.
pub(crate) fn check_forest(tracks: &[Arc<Track>]) -> Result<(), ForestError> {
    // Callers check duplicate track IDs before validating parent links.
    let indices: HashMap<_, _> = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.id, index))
        .collect();
    let mut visits = vec![Visit::Unvisited; tracks.len()];
    let mut path = Vec::new();
    for start in 0..tracks.len() {
        let mut index = start;
        loop {
            match visits[index] {
                Visit::Resolved => break,
                Visit::Active => {
                    return Err(ForestError::Cycle {
                        track: tracks[start].id,
                    });
                }
                Visit::Unvisited => {}
            }
            visits[index] = Visit::Active;
            path.push(index);
            #[cfg(test)]
            crate::model_scale_tests::parent_link();
            let Some(parent) = tracks[index].parent else {
                break;
            };
            index = *indices.get(&parent).ok_or(ForestError::MissingParent {
                track: tracks[start].id,
                parent,
            })?;
        }
        for index in path.drain(..) {
            visits[index] = Visit::Resolved;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "hierarchy_tests.rs"]
mod tests;
