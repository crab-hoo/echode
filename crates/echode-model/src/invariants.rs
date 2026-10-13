//! Rules every project snapshot keeps.
//!
//! The list is the `echode-model` crate documentation.
//! [`Project::check_invariants`] reports the first rule that fails.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::id::Id;
use crate::model::{ClipKind, Project, arrange_fault, fade_allowed, note_ord};

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
                if note.pitch > 127 || note.length.get() <= 0 || note.articulation.is_some() {
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
                if !clip.gain.is_finite() || clip.gain < 0.0 {
                    return broken(format!(
                        "clip {} gain must be finite and at least zero",
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
                    let Some(last) = absolute.checked_add(note.length.get() - 1) else {
                        return broken(format!("note {} ends past the time range", note.id));
                    };
                    let (first_bar, last_bar) = (absolute.div_euclid(bar), last.div_euclid(bar));
                    bars = Some(match bars {
                        Some((low, high)) => (low.min(first_bar), high.max(last_bar)),
                        None => (first_bar, last_bar),
                    });
                }
                if clip.kind == ClipKind::Auto {
                    let Some((low, high)) = bars else {
                        return broken(format!("auto clip {} has no notes", clip.id));
                    };
                    if start != low * bar || end != (high + 1) * bar {
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
            let referenced = self
                .tracks()
                .iter()
                .any(|track| track.clips.iter().any(|clip| clip.content == content.id));
            if !referenced {
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
mod tests {
    use super::*;
    use crate::id::Id;
    use crate::model::{
        Content, DEFAULT_FADE_FLICKS, Lane, ModelError, Note, NoteDraft, Placement, Track,
    };
    use crate::time::{Meter, Tempo, TempoMap, Tick};

    fn project(contents: Vec<Content>, tracks: Vec<Track>) -> Project {
        Project::unchecked(
            Meter::four_four(),
            TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
            contents,
            tracks,
        )
    }

    fn track(id: u128, parent: Option<u128>, clips: Vec<Placement>) -> Track {
        Track {
            id: Id::from_raw(id),
            name: format!("track {id}"),
            parent: parent.map(Id::from_raw),
            order: crate::model::sibling_order(id as u32),
            devices: vec![crate::device::test_mixer(id)],
            clips,
        }
    }

    /// A placement of `bars` 4/4 bars at bar `first`, and its content with one quarter note.
    fn clip(id: u128, kind: ClipKind, first: i64, bars: i64) -> (Content, Placement) {
        let content = Content {
            id: Id::from_raw(id + 500),
            audio: None,
            takes: Vec::new(),
            notes: vec![Note {
                id: Id::from_raw(id + 1_000),
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
                pitch: 60,
                velocity: 100,
                release: 0,
                articulation: None,
            }],
            expressions: Vec::new(),
        };
        let placement = Placement {
            id: Id::from_raw(id),
            content: content.id,
            lane: Lane::Timeline,
            start: Tick::quarters(first * 4).unwrap(),
            length: Tick::quarters(bars * 4).unwrap(),
            offset: Tick::new(0),
            kind,
            gain: 1.0,
            fade_in: DEFAULT_FADE_FLICKS,
            fade_out: DEFAULT_FADE_FLICKS,
        };
        (content, placement)
    }

    fn broken_rule(project: &Project) -> String {
        project.check_invariants().unwrap_err().to_string()
    }

    #[test]
    fn a_new_project_and_an_edited_one_keep_the_rules() {
        let (project, track) = Project::new().add_track("piano");
        let (project, _) = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::quarters(5).unwrap(),
                },
            )
            .unwrap();
        assert_eq!(Project::new().check_invariants(), Ok(()));
        assert_eq!(project.check_invariants(), Ok(()));
    }

    #[test]
    fn each_broken_rule_is_reported() {
        let (content, placement) = clip(2, ClipKind::Fixed, 0, 1);
        let reused = project(
            vec![content],
            vec![track(1, None, vec![placement]), track(2, None, Vec::new())],
        );
        assert!(broken_rule(&reused).contains("used twice"));

        let (left_content, left) = clip(4, ClipKind::Fixed, 0, 2);
        let (right_content, right) = clip(5, ClipKind::Fixed, 1, 1);
        let overlapping = project(
            vec![left_content, right_content],
            vec![track(1, None, vec![left, right])],
        );
        assert!(broken_rule(&overlapping).contains("starts before"));

        let (content, placement) = clip(2, ClipKind::Auto, 0, 2);
        let loose_auto = project(vec![content], vec![track(1, None, vec![placement])]);
        assert!(broken_rule(&loose_auto).contains("auto clip"));

        let orphan = project(Vec::new(), vec![track(1, Some(9), Vec::new())]);
        assert!(broken_rule(&orphan).contains("missing parent"));

        let cycle = project(
            Vec::new(),
            vec![track(1, Some(2), Vec::new()), track(2, Some(1), Vec::new())],
        );
        assert!(broken_rule(&cycle).contains("own ancestor"));

        let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
        content.notes.push(Note {
            id: Id::from_raw(8),
            start: Tick::quarters(-1).unwrap(),
            length: Tick::quarters(1).unwrap(),
            release: 0,
            articulation: None,
            pitch: 60,
            velocity: 100,
        });
        let unsorted = project(vec![content], vec![track(1, None, vec![placement])]);
        assert!(broken_rule(&unsorted).contains("out of start, pitch, id order"));
    }

    fn assemble(contents: Vec<Content>, tracks: Vec<Track>) -> Result<Project, ModelError> {
        Project::try_assemble(
            Meter::four_four(),
            TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
            contents,
            tracks,
        )
    }

    #[test]
    fn assembly_rejects_a_window_a_reversed_pair_an_overflow_and_a_self_take() {
        let (content, placement) = clip(2, ClipKind::Auto, 0, 2);
        let error = assemble(vec![content], vec![track(1, None, vec![placement])]).unwrap_err();
        assert!(
            matches!(error, ModelError::Invariant(ref message) if message.contains("auto clip")),
            "{error}"
        );

        let (early_content, early) = clip(4, ClipKind::Fixed, 0, 1);
        let (late_content, late) = clip(5, ClipKind::Fixed, 2, 1);
        let error = assemble(
            vec![early_content, late_content],
            vec![track(1, None, vec![late, early])],
        )
        .unwrap_err();
        assert!(
            matches!(error, ModelError::Invariant(ref message) if message.contains("starts before")),
            "{error}"
        );

        let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
        content.notes[0].start = Tick::new(i64::MAX - 1);
        content.notes[0].length = Tick::new(2);
        let error = assemble(vec![content], vec![track(1, None, vec![placement])]).unwrap_err();
        assert_eq!(error, ModelError::InvalidNote);

        let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
        content.takes = vec![content.id];
        let error = assemble(vec![content], vec![track(1, None, vec![placement])]).unwrap_err();
        assert_eq!(error, ModelError::DuplicateId);
    }

    #[test]
    fn the_shared_check_sees_gain_fade_overflow_takes_and_mixer_ends() {
        let (content, mut placement) = clip(2, ClipKind::Fixed, 0, 1);
        placement.gain = -1.0;
        let negative_gain = project(vec![content], vec![track(1, None, vec![placement])]);
        assert!(broken_rule(&negative_gain).contains("gain"));

        let (content, mut placement) = clip(2, ClipKind::Fixed, 0, 1);
        placement.fade_in = -1;
        let negative_fade = project(vec![content], vec![track(1, None, vec![placement])]);
        assert!(broken_rule(&negative_fade).contains("fade"));

        let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
        content.notes[0].start = Tick::new(i64::MAX - 1);
        content.notes[0].length = Tick::new(2);
        let overflow = project(vec![content], vec![track(1, None, vec![placement])]);
        assert!(broken_rule(&overflow).contains("ends past the time range"));

        let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
        content.takes = vec![content.id];
        let self_take = project(vec![content], vec![track(1, None, vec![placement])]);
        assert!(broken_rule(&self_take).contains("lists itself as a take"));

        let (content, placement) = clip(2, ClipKind::Fixed, 0, 1);
        let mut hosted = track(1, None, vec![placement]);
        hosted.devices[0].parameters[0].max = 5.0;
        let wide = project(vec![content], vec![hosted]);
        assert!(broken_rule(&wide).contains("outside its range"));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "lists itself as a take")]
    fn a_broken_snapshot_panics_where_apply_checks() {
        let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
        content.takes = vec![content.id];
        let broken = project(vec![content], vec![track(1, None, vec![placement])]);
        broken.debug_assert_invariants();
    }
}
