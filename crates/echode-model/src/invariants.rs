//! Rules every project snapshot keeps.
//!
//! The list is the `echode-model` crate documentation.
//! [`Project::check_invariants`] reports the first rule that fails.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::id::Id;
use crate::model::{
    CLIP_GAIN_MAX, ClipKind, Project, arrange_fault, clip_gain_allowed, fade_allowed, note_ord,
};

/// The first rule a snapshot broke, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvariantError(String);

impl fmt::Display for InvariantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InvariantError {}

fn broken(message: String) -> Result<(), InvariantError> {
    Err(InvariantError(message))
}

impl Project {
    /// Reports the first broken rule from the crate-level invariant list.
    pub fn check_invariants(&self) -> Result<(), InvariantError> {
        let Some(bar) = self.meter().bar_ticks().map(|bar| bar.get()) else {
            return broken("the meter has no whole-tick bar".into());
        };
        if self
            .contents()
            .windows(2)
            .any(|pair| pair[0].id > pair[1].id)
        {
            return broken("contents are out of content id order".into());
        }
        let referenced = crate::model::referenced_contents(self.tracks());
        let mut ids = HashSet::new();
        let mut parents = HashMap::new();
        let mut auto_counts: HashMap<Id, usize> = HashMap::new();
        for content in self.contents() {
            if !ids.insert(content.id) {
                return broken(format!("id {} is used twice", content.id));
            }
            if content.takes.contains(&content.id) {
                return broken(format!("content {} lists itself as a take", content.id));
            }
            for note in &content.notes {
                if !ids.insert(note.id) {
                    return broken(format!("id {} is used twice", note.id));
                }
                if note.pitch > crate::MIDI7_MAX
                    || note.length.get() <= 0
                    || note.articulation.is_some()
                {
                    return broken(format!("note {} is not usable", note.id));
                }
                if note.start.get().checked_add(note.length.get()).is_none() {
                    return broken(format!("note {} ends past the time range", note.id));
                }
            }
            if !content.expressions.is_empty() {
                return broken(format!(
                    "content {} has expression curves before they can be edited",
                    content.id
                ));
            }
            if let Some(pair) = content
                .notes
                .windows(2)
                .find(|pair| note_ord(&pair[0], &pair[1]).is_gt())
            {
                return broken(format!(
                    "notes {} then {} in content {} are out of start, pitch, id order",
                    pair[0].id, pair[1].id, content.id
                ));
            }
        }
        for track in self.tracks() {
            if !ids.insert(track.id) {
                return broken(format!("id {} is used twice", track.id));
            }
            let Some(mixer) = track.devices.last() else {
                return broken(format!("track {} does not have one mixer", track.name));
            };
            if mixer.kind != crate::device::DeviceKind::Mixer || mixer.plugin.is_some() {
                return broken(format!("track {} does not have one mixer", track.name));
            }
            if mixer.bypassed || mixer.offline || mixer.state.is_some() {
                return broken(format!(
                    "track {} mixer is bypassed, offline or holds plugin state",
                    track.name
                ));
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
                return broken(format!(
                    "track {} has a plugin device without a CLAP id",
                    track.name
                ));
            }
            for device in &track.devices {
                if !ids.insert(device.id) {
                    return broken(format!("id {} is used twice", device.id));
                }
            }
            if mixer.parameters.len() != crate::device::ParamKey::ORDER.len() {
                return broken(format!(
                    "track {} mixer does not have its four parameters",
                    track.name
                ));
            }
            for (parameter, key) in mixer.parameters.iter().zip(crate::device::ParamKey::ORDER) {
                if !ids.insert(parameter.id) {
                    return broken(format!("id {} is used twice", parameter.id));
                }
                let (min, max, default) = key.ends();
                if parameter.key != key
                    || parameter.min.to_bits() != min.to_bits()
                    || parameter.max.to_bits() != max.to_bits()
                    || parameter.default.to_bits() != default.to_bits()
                    || !key.allows(parameter.value)
                {
                    return broken(format!(
                        "parameter {} on {} is outside its range",
                        parameter.id, track.name
                    ));
                }
            }
            parents.insert(track.id, track.parent);
            let mut previous_end = None;
            for clip in &track.clips {
                if !ids.insert(clip.id) {
                    return broken(format!("id {} is used twice", clip.id));
                }
                let start = clip.start.get();
                let Some(end) = start.checked_add(clip.length.get()) else {
                    return broken(format!("clip {} ends past the time range", clip.id));
                };
                let Some(content) = self.content(clip.content) else {
                    return broken(format!("clip {} names a missing content", clip.id));
                };
                let Some(origin) = start.checked_sub(clip.offset.get()) else {
                    return broken(format!("clip {} has no origin", clip.id));
                };
                if clip.length.get() <= 0 {
                    return broken(format!("clip {} is empty", clip.id));
                }
                if !clip_gain_allowed(clip.gain) {
                    return broken(format!(
                        "clip {} gain must be finite, at least zero, and at most {CLIP_GAIN_MAX}",
                        clip.id
                    ));
                }
                if !fade_allowed(clip.fade_in) || !fade_allowed(clip.fade_out) {
                    return broken(format!(
                        "clip {} fade is negative or does not fit in a frame count",
                        clip.id
                    ));
                }
                if previous_end.is_some_and(|previous| start < previous) {
                    return broken(format!(
                        "clip {} starts before the previous clip on {} ends",
                        clip.id, track.name
                    ));
                }
                previous_end = Some(end);
                let mut bars: Option<(i64, i64)> = None;
                if clip.kind == ClipKind::Auto {
                    *auto_counts.entry(clip.content).or_insert(0) += 1;
                }
                for note in &content.notes {
                    let Some(absolute) = origin.checked_add(note.start.get()) else {
                        return broken(format!("note {} starts past the time range", note.id));
                    };
                    let Some(span) = note.length.get().checked_sub(1) else {
                        return broken(format!("note {} ends past the time range", note.id));
                    };
                    let Some(_) = absolute.checked_add(span) else {
                        return broken(format!("note {} ends past the time range", note.id));
                    };
                    let local_last = note.start.get().checked_add(span).ok_or_else(|| {
                        InvariantError(format!("note {} ends past the content time range", note.id))
                    })?;
                    let (first_bar, last_bar) =
                        (note.start.get().div_euclid(bar), local_last.div_euclid(bar));
                    bars = Some(match bars {
                        Some((low, high)) => (low.min(first_bar), high.max(last_bar)),
                        None => (first_bar, last_bar),
                    });
                }
                if clip.kind == ClipKind::Auto {
                    let Some((low, high)) = bars else {
                        return broken(format!("auto clip {} has no notes", clip.id));
                    };
                    let Some(covered_start) = low
                        .checked_mul(bar)
                        .and_then(|local| origin.checked_add(local))
                    else {
                        return broken(format!("auto clip {} starts past the time range", clip.id));
                    };
                    let Some(covered_end) = high
                        .checked_add(1)
                        .and_then(|next| next.checked_mul(bar))
                        .and_then(|local| origin.checked_add(local))
                    else {
                        return broken(format!("auto clip {} ends past the time range", clip.id));
                    };
                    if start != covered_start || end != covered_end {
                        return broken(format!(
                            "auto clip {} spans {start}..{end}, but its notes cover bars {low}..={high}",
                            clip.id
                        ));
                    }
                }
            }
        }
        if auto_counts.values().any(|count| *count > 1) {
            return broken("a content has more than one auto placement".into());
        }
        for content in self.contents() {
            if !referenced.contains(&content.id) {
                return broken(format!("content {} has no placement", content.id));
            }
            if let Some(audio) = &content.audio
                && audio.check().is_err()
            {
                return broken(format!(
                    "content {} has an unusable audio reference",
                    content.id
                ));
            }
        }
        for (&track, &parent) in &parents {
            let mut cursor = parent;
            let mut steps = 0;
            while let Some(id) = cursor {
                let Some(next) = parents.get(&id) else {
                    return broken(format!("track {track} names a missing parent {id}"));
                };
                steps += 1;
                if id == track || steps > parents.len() {
                    return broken(format!("track {track} is its own ancestor"));
                }
                cursor = *next;
            }
        }
        if let Some(message) = arrange_fault(self.tracks()) {
            return broken(message);
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
