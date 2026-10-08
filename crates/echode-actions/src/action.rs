//! Model-changing actions. UI-only actions (zoom, panes) stay in Swift.

use std::fmt;

use echode_model::{Id, Meter, ModelError, NoteDraft, Project, Tempo, Tick};

/// What a caller asks for. Created ids are filled in when the command is performed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    AddTrack {
        name: String,
    },
    AddChild {
        parent: Id,
        name: String,
    },
    SetParent {
        track: Id,
        parent: Option<Id>,
    },
    SetMeter {
        meter: Meter,
    },
    SetTempo {
        at: Tick,
        tempo: Tempo,
    },
    InsertNote {
        track: Id,
        draft: NoteDraft,
    },
    DeleteNote {
        track: Id,
        note: Id,
    },
    ResizeClip {
        track: Id,
        clip: Id,
        start: Tick,
        length: Tick,
    },
}

/// A committed edit. Replaying these actions rebuilds the same project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    AddTrack {
        id: Id,
        name: String,
    },
    AddChild {
        id: Id,
        parent: Id,
        name: String,
    },
    SetParent {
        track: Id,
        parent: Option<Id>,
    },
    SetMeter {
        meter: Meter,
    },
    SetTempo {
        at: Tick,
        tempo: Tempo,
    },
    InsertNote {
        track: Id,
        note: Id,
        clip: Id,
        draft: NoteDraft,
    },
    DeleteNote {
        track: Id,
        note: Id,
    },
    ResizeClip {
        track: Id,
        clip: Id,
        start: Tick,
        length: Tick,
    },
}

/// Names shown in the command palette and handed to the on-device model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionSpec {
    pub id: &'static str,
    pub name_ja: &'static str,
    pub name_en: &'static str,
    pub description_ja: &'static str,
    pub description_en: &'static str,
}

const SPECS: [ActionSpec; 8] = [
    ActionSpec {
        id: "track.add",
        name_ja: "トラックを追加",
        name_en: "Add a track",
        description_ja: "空のトラックを末尾に追加する",
        description_en: "Add an empty track at the end",
    },
    ActionSpec {
        id: "project.set_meter",
        name_ja: "拍子を変える",
        name_en: "Change the meter",
        description_ja: "小節の長さを変える。クリップがある間は変えられない",
        description_en: "Change the bar length. This is refused while any clip exists",
    },
    ActionSpec {
        id: "project.set_tempo",
        name_ja: "テンポを変える",
        name_en: "Change the tempo",
        description_ja: "指定した位置から一定のテンポを置く",
        description_en: "Place a constant tempo starting at a tick",
    },
    ActionSpec {
        id: "note.insert",
        name_ja: "ノートを置く",
        name_en: "Place a note",
        description_ja: "ノートを置き、必要なら自動クリップを作るか伸ばす",
        description_en: "Place a note, creating or extending an auto clip when needed",
    },
    ActionSpec {
        id: "note.delete",
        name_ja: "ノートを消す",
        name_en: "Delete a note",
        description_ja: "ノートを消す。空になった自動クリップも消える",
        description_en: "Delete a note. An auto clip with no notes left is removed",
    },
    ActionSpec {
        id: "clip.resize",
        name_ja: "クリップの範囲を変える",
        name_en: "Resize a clip",
        description_ja: "範囲を変え、クリップを固定する。ノートの時刻は動かない",
        description_en: "Change the range and fix the clip. Notes stay at the same time",
    },
    ActionSpec {
        id: "track.add_child",
        name_ja: "子トラックを追加",
        name_en: "Add a child track",
        description_ja: "フォルダの中にトラックを置く。深さに上限はない",
        description_en: "Place a track inside a folder. Nesting has no fixed depth",
    },
    ActionSpec {
        id: "track.set_parent",
        name_ja: "フォルダを変える",
        name_en: "Move a track into a folder",
        description_ja: "トラックの親を変える。自分の子孫の中には入れない",
        description_en: "Change a track's parent. A track cannot move into its own subtree",
    },
];

/// Every model action the palette can name.
pub fn catalog() -> &'static [ActionSpec] {
    &SPECS
}

impl Action {
    pub(crate) fn from_command(command: Command) -> Self {
        match command {
            Command::AddTrack { name } => Self::AddTrack {
                id: Id::generate(),
                name,
            },
            Command::AddChild { parent, name } => Self::AddChild {
                id: Id::generate(),
                parent,
                name,
            },
            Command::SetParent { track, parent } => Self::SetParent { track, parent },
            Command::SetMeter { meter } => Self::SetMeter { meter },
            Command::SetTempo { at, tempo } => Self::SetTempo { at, tempo },
            Command::InsertNote { track, draft } => Self::InsertNote {
                track,
                note: Id::generate(),
                clip: Id::generate(),
                draft,
            },
            Command::DeleteNote { track, note } => Self::DeleteNote { track, note },
            Command::ResizeClip {
                track,
                clip,
                start,
                length,
            } => Self::ResizeClip {
                track,
                clip,
                start,
                length,
            },
        }
    }

    pub(crate) fn apply(&self, project: &Project) -> Result<Project, ActionError> {
        match self {
            Self::AddTrack { id, name } => project
                .add_track_with_id(*id, name.clone())
                .map_err(Into::into),
            Self::AddChild { id, parent, name } => project
                .add_child_with_id(*id, *parent, name.clone())
                .map_err(Into::into),
            Self::SetParent { track, parent } => {
                project.set_parent(*track, *parent).map_err(Into::into)
            }
            Self::SetMeter { meter } => project.with_meter(*meter).map_err(Into::into),
            Self::SetTempo { at, tempo } => Ok(project.with_tempo(*at, *tempo)),
            Self::InsertNote {
                track,
                note,
                clip,
                draft,
            } => project
                .insert_note_with_id(*track, *note, *clip, *draft)
                .map_err(Into::into),
            Self::DeleteNote { track, note } => {
                project.delete_note(*track, *note).map_err(Into::into)
            }
            Self::ResizeClip {
                track,
                clip,
                start,
                length,
            } => project
                .resize_clip(*track, *clip, *start, *length)
                .map_err(Into::into),
        }
    }

    /// Palette entry for this action.
    pub fn spec(&self) -> &'static ActionSpec {
        match self {
            Self::AddTrack { .. } => &SPECS[0],
            Self::AddChild { .. } => &SPECS[6],
            Self::SetParent { .. } => &SPECS[7],
            Self::SetMeter { .. } => &SPECS[1],
            Self::SetTempo { .. } => &SPECS[2],
            Self::InsertNote { .. } => &SPECS[3],
            Self::DeleteNote { .. } => &SPECS[4],
            Self::ResizeClip { .. } => &SPECS[5],
        }
    }
}

impl From<ModelError> for ActionError {
    fn from(error: ModelError) -> Self {
        Self::Model(error)
    }
}

/// Why a command or a journal replay stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    Model(ModelError),
    NothingToUndo,
    NothingToRedo,
    CorruptJournal,
}

impl fmt::Display for ActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(error) => write!(formatter, "{error}"),
            Self::NothingToUndo => formatter.write_str("nothing to undo"),
            Self::NothingToRedo => formatter.write_str("nothing to redo"),
            Self::CorruptJournal => formatter.write_str("journal entry is not valid"),
        }
    }
}

impl std::error::Error for ActionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Model(error) => Some(error),
            Self::NothingToUndo | Self::NothingToRedo | Self::CorruptJournal => None,
        }
    }
}
