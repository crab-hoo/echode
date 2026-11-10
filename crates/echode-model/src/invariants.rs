//! Rules every project snapshot keeps.
//!
//! The list is the `echode-model` crate documentation.
//! [`Project::check_invariants`] reports the first rule that fails.

use std::collections::{HashMap, HashSet};

use crate::error::InvariantError;
use crate::id::Id;
use crate::model::{ClipKind, Project, arrange_fault, clip_gain_allowed, fade_allowed, note_ord};

impl Project {
    /// Reports the first broken rule from the crate-level invariant list.
    pub fn check_invariants(&self) -> Result<(), InvariantError> {
        let Some(bar) = self.meter().bar_ticks().map(|bar| bar.get()) else {
            return Err(InvariantError::InvalidMeter);
        };
        if self
            .contents()
            .windows(2)
            .any(|pair| pair[0].id > pair[1].id)
        {
            return Err(InvariantError::ContentOrder);
        }
        let referenced = crate::model::referenced_contents(self.tracks());
        let mut ids = HashSet::new();
        let mut auto_counts: HashMap<Id, usize> = HashMap::new();
        let mut content_bounds = HashMap::new();
        for content in self.contents() {
            if !ids.insert(content.id) {
                return Err(InvariantError::DuplicateId { id: content.id });
            }
            if content.takes.contains(&content.id) {
                return Err(InvariantError::SelfTake {
                    content: content.id,
                });
            }
            let mut bounds: Option<(i64, i64)> = None;
            for note in &content.notes {
                #[cfg(test)]
                crate::model_scale_tests::note_validation();
                if !ids.insert(note.id) {
                    return Err(InvariantError::DuplicateId { id: note.id });
                }
                if note.pitch > crate::MIDI7_MAX
                    || note.length.get() <= 0
                    || note.articulation.is_some()
                {
                    return Err(InvariantError::InvalidNote { note: note.id });
                }
                let Some(end) = note.start.get().checked_add(note.length.get()) else {
                    return Err(InvariantError::NoteEnd { note: note.id });
                };
                let last = end - 1;
                bounds = Some(match bounds {
                    Some((first, previous_last)) => {
                        (first.min(note.start.get()), previous_last.max(last))
                    }
                    None => (note.start.get(), last),
                });
            }
            if !content.expressions.is_empty() {
                return Err(InvariantError::Expressions {
                    content: content.id,
                });
            }
            if let Some((first, second)) = content
                .notes
                .iter()
                .zip(content.notes.iter().skip(1))
                .find(|(first, second)| note_ord(first, second).is_gt())
            {
                return Err(InvariantError::NoteOrder {
                    content: content.id,
                    first: first.id,
                    second: second.id,
                });
            }
            content_bounds.insert(content.id, (content, bounds));
        }
        for track in self.tracks() {
            if !ids.insert(track.id) {
                return Err(InvariantError::DuplicateId { id: track.id });
            }
            let Some(mixer) = track.devices.last() else {
                return Err(InvariantError::MissingMixer {
                    track: track.id,
                    name: track.name.clone(),
                });
            };
            if mixer.kind != crate::device::DeviceKind::Mixer || mixer.plugin.is_some() {
                return Err(InvariantError::MissingMixer {
                    track: track.id,
                    name: track.name.clone(),
                });
            }
            if mixer.bypassed || mixer.offline || mixer.state.is_some() {
                return Err(InvariantError::InvalidMixer {
                    track: track.id,
                    name: track.name.clone(),
                });
            }
            if track.devices[..track.devices.len() - 1]
                .iter()
                .any(|device| {
                    device.kind != crate::device::DeviceKind::Clap
                        || device
                            .plugin
                            .as_deref()
                            .is_none_or(|plugin| !crate::device::valid_plugin_id(plugin))
                        || !device.parameters.is_empty()
                })
            {
                return Err(InvariantError::InvalidPlugin {
                    track: track.id,
                    name: track.name.clone(),
                });
            }
            for device in &track.devices {
                if !ids.insert(device.id) {
                    return Err(InvariantError::DuplicateId { id: device.id });
                }
            }
            if mixer.parameters.len() != crate::device::ParamKey::ORDER.len() {
                return Err(InvariantError::MixerParameters {
                    track: track.id,
                    name: track.name.clone(),
                });
            }
            for (parameter, key) in mixer.parameters.iter().zip(crate::device::ParamKey::ORDER) {
                if !ids.insert(parameter.id) {
                    return Err(InvariantError::DuplicateId { id: parameter.id });
                }
                let (min, max, default) = key.ends();
                if parameter.key != key
                    || parameter.min.to_bits() != min.to_bits()
                    || parameter.max.to_bits() != max.to_bits()
                    || parameter.default.to_bits() != default.to_bits()
                    || !key.allows(parameter.value)
                {
                    return Err(InvariantError::ParameterRange {
                        track: track.id,
                        track_name: track.name.clone(),
                        parameter: parameter.id,
                    });
                }
            }
            let mut previous_end = None;
            for clip in &track.clips {
                if !ids.insert(clip.id) {
                    return Err(InvariantError::DuplicateId { id: clip.id });
                }
                let start = clip.start.get();
                let Some(end) = start.checked_add(clip.length.get()) else {
                    return Err(InvariantError::ClipEnd { clip: clip.id });
                };
                let Some((content, bounds)) = content_bounds.get(&clip.content) else {
                    return Err(InvariantError::MissingContent { clip: clip.id });
                };
                let Some(origin) = start.checked_sub(clip.offset.get()) else {
                    return Err(InvariantError::ClipOrigin { clip: clip.id });
                };
                if clip.length.get() <= 0 {
                    return Err(InvariantError::EmptyClip { clip: clip.id });
                }
                if !clip_gain_allowed(clip.gain) {
                    return Err(InvariantError::ClipGain { clip: clip.id });
                }
                if !fade_allowed(clip.fade_in) || !fade_allowed(clip.fade_out) {
                    return Err(InvariantError::ClipFade { clip: clip.id });
                }
                if previous_end.is_some_and(|previous| start < previous) {
                    return Err(InvariantError::ClipOverlap {
                        clip: clip.id,
                        track: track.id,
                        track_name: track.name.clone(),
                    });
                }
                previous_end = Some(end);
                if clip.kind == ClipKind::Auto {
                    *auto_counts.entry(clip.content).or_insert(0) += 1;
                }
                // Validated local extrema suffice for every note at this origin.
                // Only invalid placements rescan to preserve the first-note diagnostic.
                if bounds.is_some_and(|(first, last)| {
                    origin.checked_add(first).is_none() || origin.checked_add(last).is_none()
                }) {
                    for note in &content.notes {
                        #[cfg(test)]
                        crate::model_scale_tests::note_validation();
                        let Some(absolute) = origin.checked_add(note.start.get()) else {
                            return Err(InvariantError::NoteStart { note: note.id });
                        };
                        if absolute.checked_add(note.length.get() - 1).is_none() {
                            return Err(InvariantError::NoteEnd { note: note.id });
                        }
                    }
                }
                let bars =
                    bounds.map(|(first, last)| (first.div_euclid(bar), last.div_euclid(bar)));
                if clip.kind == ClipKind::Auto {
                    let Some((low, high)) = bars else {
                        return Err(InvariantError::EmptyAuto { clip: clip.id });
                    };
                    let Some(covered_start) = low
                        .checked_mul(bar)
                        .and_then(|local| origin.checked_add(local))
                    else {
                        return Err(InvariantError::AutoStart { clip: clip.id });
                    };
                    let Some(covered_end) = high
                        .checked_add(1)
                        .and_then(|next| next.checked_mul(bar))
                        .and_then(|local| origin.checked_add(local))
                    else {
                        return Err(InvariantError::AutoEnd { clip: clip.id });
                    };
                    if start != covered_start || end != covered_end {
                        return Err(InvariantError::AutoBounds {
                            clip: clip.id,
                            start,
                            end,
                            low,
                            high,
                        });
                    }
                }
            }
        }
        if let Some((&content, _)) = auto_counts.iter().find(|(_, count)| **count > 1) {
            return Err(InvariantError::SharedAutoContent { content });
        }
        for content in self.contents() {
            if !referenced.contains(&content.id) {
                return Err(InvariantError::UnplacedContent {
                    content: content.id,
                });
            }
            if let Some(audio) = &content.audio
                && audio.check().is_err()
            {
                return Err(InvariantError::InvalidAudio {
                    content: content.id,
                });
            }
        }
        crate::hierarchy::check_forest(self.tracks()).map_err(|error| match error {
            crate::hierarchy::ForestError::MissingParent { track, parent } => {
                InvariantError::MissingParent { track, parent }
            }
            crate::hierarchy::ForestError::Cycle { track } => InvariantError::TrackCycle { track },
        })?;
        if let Some(error) = arrange_fault(self.tracks()) {
            return Err(InvariantError::Order(Box::new(error)));
        }
        Ok(())
    }

    /// Panics in debug builds when a rule is broken. Release builds do nothing.
    ///
    /// The scan walks the whole project, so the audio thread must not call it.
    /// [`crate::model::Project::try_assemble`] still checks in every build.
    pub fn debug_assert_invariants(&self) {
        #[cfg(debug_assertions)]
        if let Err(broken) = self.check_invariants() {
            panic!("{broken}");
        }
    }
}

#[cfg(test)]
#[path = "invariants_tests.rs"]
mod tests;
