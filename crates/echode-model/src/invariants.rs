//! Rules every project snapshot keeps.
//!
//! Tests call [`Project::check_invariants`] after every edit. Two rules are not
//! checked yet because the model still breaks them: children directly after their
//! parent (audit MOD-3) and notes in start order inside a clip (audit MOD-4).
//! Add each one here when its item is fixed.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::model::{ClipKind, Project};

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
    /// Checks that ids are unique, clips are sorted and apart, notes are usable,
    /// auto clips span exactly the bars of their notes, and parents form a forest.
    pub fn check_invariants(&self) -> Result<(), InvariantError> {
        let Some(bar) = self.meter().bar_ticks().map(|bar| bar.get()) else {
            return broken("the meter has no whole-tick bar".into());
        };
        let mut ids = HashSet::new();
        let mut parents = HashMap::new();
        for track in self.tracks() {
            if !ids.insert(track.id) {
                return broken(format!("id {} is used twice", track.id));
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
                if clip.length.get() <= 0 {
                    return broken(format!("clip {} is empty", clip.id));
                }
                if previous_end.is_some_and(|previous| start < previous) {
                    return broken(format!(
                        "clip {} starts before the previous clip on {} ends",
                        clip.id, track.name
                    ));
                }
                previous_end = Some(end);
                let mut bars: Option<(i64, i64)> = None;
                for note in &clip.notes {
                    if !ids.insert(note.id) {
                        return broken(format!("id {} is used twice", note.id));
                    }
                    if note.pitch > 127 || note.velocity > 127 || note.length.get() <= 0 {
                        return broken(format!("note {} is not usable", note.id));
                    }
                    let Some(absolute) = start.checked_add(note.start.get()) else {
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::Id;
    use crate::model::{Clip, Note, NoteDraft, Track};
    use crate::time::{Meter, Tempo, TempoMap, Tick};

    fn project(tracks: Vec<Track>) -> Project {
        Project::unchecked(
            Meter::four_four(),
            TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
            tracks,
        )
    }

    fn track(id: u128, parent: Option<u128>, clips: Vec<Clip>) -> Track {
        Track {
            id: Id::from_raw(id),
            name: format!("track {id}"),
            parent: parent.map(Id::from_raw),
            clips,
        }
    }

    /// A clip of `bars` 4/4 bars starting at bar `first`, with one quarter note at its start.
    fn clip(id: u128, kind: ClipKind, first: i64, bars: i64) -> Clip {
        Clip {
            id: Id::from_raw(id),
            start: Tick::quarters(first * 4).unwrap(),
            length: Tick::quarters(bars * 4).unwrap(),
            kind,
            notes: vec![Note {
                id: Id::from_raw(id + 1_000),
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
                pitch: 60,
                velocity: 100,
            }],
        }
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
        let reused = project(vec![
            track(1, None, vec![clip(2, ClipKind::Fixed, 0, 1)]),
            track(2, None, Vec::new()),
        ]);
        assert!(broken_rule(&reused).contains("used twice"));

        let overlapping = project(vec![track(
            1,
            None,
            vec![
                clip(2, ClipKind::Fixed, 0, 2),
                clip(3, ClipKind::Fixed, 1, 1),
            ],
        )]);
        assert!(broken_rule(&overlapping).contains("starts before"));

        let loose_auto = project(vec![track(1, None, vec![clip(2, ClipKind::Auto, 0, 2)])]);
        assert!(broken_rule(&loose_auto).contains("auto clip"));

        let orphan = project(vec![track(1, Some(9), Vec::new())]);
        assert!(broken_rule(&orphan).contains("missing parent"));

        let cycle = project(vec![
            track(1, Some(2), Vec::new()),
            track(2, Some(1), Vec::new()),
        ]);
        assert!(broken_rule(&cycle).contains("own ancestor"));
    }
}
