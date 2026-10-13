//! Model-changing actions. UI-only actions (zoom, panes) stay in Swift.
//!
//! [`define_actions!`] is the only list of model actions. It builds [`Command`],
//! [`Action`], the journal and API `type` tag, and the catalog.

use std::fmt;

use echode_model::{
    AudioRef, ChangeSet, Id, Meter, MixerIds, ModelError, NoteDraft, NoteGeometry, Project, Tempo,
    Tick,
};
use serde::{Deserialize, Serialize};

/// Names shown in the command palette and handed to the on-device model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionSpec {
    pub id: &'static str,
    pub name_ja: &'static str,
    pub name_en: &'static str,
    pub description_ja: &'static str,
    pub description_en: &'static str,
}

impl ActionSpec {
    /// JSON Schema of this action's request, including a const `type`.
    pub fn schema(&self) -> serde_json::Value {
        request_schema(self.id)
    }
}

fn schema_value<T: schemars::JsonSchema>(id: &str) -> serde_json::Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).expect("action schema is json");
    let object = value.as_object_mut().expect("action schema is an object");
    let properties = object
        .entry("properties")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    properties
        .as_object_mut()
        .expect("schema properties")
        .insert("type".to_string(), serde_json::json!({ "const": id }));
    match object.get_mut("required") {
        Some(required) => {
            let required = required
                .as_array_mut()
                .expect("schema required is an array");
            if !required.iter().any(|item| item == "type") {
                required.insert(0, serde_json::json!("type"));
            }
        }
        None => {
            object.insert("required".to_string(), serde_json::json!(["type"]));
        }
    }
    value
}

macro_rules! define_actions {
    ($(
        $variant:ident {
            id: $id:literal,
            name_ja: $name_ja:literal,
            name_en: $name_en:literal,
            description_ja: $description_ja:literal,
            description_en: $description_en:literal,
            command {
                $($(#[$cattr:meta])* $cfield:ident : $ctype:ty),* $(,)?
            }
            action {
                $($(#[$aattr:meta])* $afield:ident : $atype:ty),* $(,)?
            }
            commit $commit:block
            edit $edit:expr
        }
    )*) => {
        /// What a caller asks for. Created ids are filled in when the command is performed.
        #[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
        #[serde(tag = "type", deny_unknown_fields)]
        pub enum Command {
            $(
                #[serde(rename = $id)]
                $variant {
                    $(
                        $(#[$cattr])*
                        $cfield: $ctype,
                    )*
                },
            )*
        }

        /// A committed edit. Replaying these actions rebuilds the same project.
        #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
        #[serde(tag = "type", deny_unknown_fields)]
        pub enum Action {
            $(
                #[serde(rename = $id)]
                $variant {
                    $(
                        $(#[$aattr])*
                        $afield: $atype,
                    )*
                },
            )*
        }

        #[allow(dead_code)]
        mod command_schema {
            use echode_model::{Id, Meter, NoteDraft, Tempo, Tick};
            use schemars::JsonSchema;
            use serde::Serialize;

            $(
                #[derive(Serialize, JsonSchema)]
                pub struct $variant {
                    $(
                        $(#[$cattr])*
                        pub $cfield: $ctype,
                    )*
                }
            )*
        }

        fn request_schema(id: &str) -> serde_json::Value {
            match id {
                $($id => schema_value::<command_schema::$variant>($id),)*
                _ => panic!("unknown action {id}"),
            }
        }

        /// Every model action the palette can name.
        pub fn catalog() -> &'static [ActionSpec] {
            const CATALOG: &[ActionSpec] = &[
                $(
                    ActionSpec {
                        id: $id,
                        name_ja: $name_ja,
                        name_en: $name_en,
                        description_ja: $description_ja,
                        description_en: $description_en,
                    },
                )*
            ];
            CATALOG
        }

        impl Action {
            pub(crate) fn from_command(command: Command) -> Self {
                match command {
                    $(
                        Command::$variant { $($cfield),* } => $commit,
                    )*
                }
            }

            fn edit(&self, project: &Project) -> Result<Project, ActionError> {
                match self {
                    $(
                        Self::$variant { $($afield),* } => ($edit)(project),
                    )*
                }
            }

            /// Palette entry for this action.
            pub fn spec(&self) -> &'static ActionSpec {
                let id = match self {
                    $(Self::$variant { .. } => $id,)*
                };
                catalog()
                    .iter()
                    .find(|spec| spec.id == id)
                    .expect("every action is in the catalog")
            }
        }
    };
}

define_actions! {
    AddTrack {
        id: "track.add",
        name_ja: "トラックを追加",
        name_en: "Add a track",
        description_ja: "空のトラックを末尾に追加する",
        description_en: "Add an empty track at the end",
        command {
            name: String,
        }
        action {
            id: Id,
            name: String,
            device: Id,
            gain: Id,
            pan: Id,
            mute: Id,
            solo: Id,
        }
        commit {
            let mixer = MixerIds::generate();
            Action::AddTrack {
                id: Id::generate(),
                name,
                device: mixer.device,
                gain: mixer.gain,
                pan: mixer.pan,
                mute: mixer.mute,
                solo: mixer.solo,
            }
        }
        edit |project: &Project| {
            project
                .add_track_with_id(
                    *id,
                    name.clone(),
                    MixerIds {
                        device: *device,
                        gain: *gain,
                        pan: *pan,
                        mute: *mute,
                        solo: *solo,
                    },
                )
                .map_err(Into::into)
        }
    }
    SetMeter {
        id: "project.set_meter",
        name_ja: "拍子を変える",
        name_en: "Change the meter",
        description_ja: "小節の長さを変える。クリップがある間は変えられない",
        description_en: "Change the bar length. This is refused while any clip exists",
        command {
            #[serde(flatten)]
            meter: Meter,
        }
        action {
            #[serde(flatten)]
            meter: Meter,
        }
        commit {
            Action::SetMeter { meter }
        }
        edit |project: &Project| {
            project.with_meter(*meter).map_err(Into::into)
        }
    }
    SetTempo {
        id: "project.set_tempo",
        name_ja: "テンポを変える",
        name_en: "Change the tempo",
        description_ja: "指定した位置から一定のテンポを置く",
        description_en: "Place a constant tempo starting at a tick",
        command {
            at: Tick,
            #[serde(flatten)]
            tempo: Tempo,
        }
        action {
            at: Tick,
            #[serde(flatten)]
            tempo: Tempo,
        }
        commit {
            Action::SetTempo { at, tempo }
        }
        edit |project: &Project| {
            Ok(project.with_tempo(*at, *tempo))
        }
    }
    InsertNote {
        id: "note.insert",
        name_ja: "ノートを置く",
        name_en: "Place a note",
        description_ja: "ノートを置き、必要なら自動クリップを作るか伸ばす",
        description_en: "Place a note, creating or extending an auto clip when needed",
        command {
            track: Id,
            #[serde(flatten)]
            draft: NoteDraft,
        }
        action {
            track: Id,
            note: Id,
            clip: Id,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            content: Option<Id>,
            #[serde(flatten)]
            draft: NoteDraft,
        }
        commit {
            Action::InsertNote {
                track,
                note: Id::generate(),
                clip: Id::generate(),
                content: Some(Id::generate()),
                draft,
            }
        }
        edit |project: &Project| {
            let content = (*content).unwrap_or_else(|| Id::from_raw(clip.raw() ^ 1));
            project
                .insert_note_with_id(*track, *note, *clip, content, *draft)
                .map_err(Into::into)
        }
    }
    EditNote {
        id: "note.edit",
        name_ja: "ノートを編集する",
        name_en: "Edit a note",
        description_ja: "ノートの音高、開始、長さを変える。ID と表現は保つ",
        description_en: "Change pitch, start, and length, preserving identity and expression",
        command { track: Id, clip: Id, note: Id, pitch: u8, start: Tick, length: Tick }
        action { track: Id, clip: Id, note: Id, pitch: u8, start: Tick, length: Tick }
        commit { Action::EditNote { track, clip, note, pitch, start, length } }
        edit |project: &Project| {
            project.edit_note(*track, *clip, *note, NoteGeometry {
                pitch: *pitch, start: *start, length: *length,
            }).map_err(Into::into)
        }
    }
    DeleteNote {
        id: "note.delete",
        name_ja: "ノートを消す",
        name_en: "Delete a note",
        description_ja: "ノートを消す。空になった自動クリップも消える",
        description_en: "Delete a note. An auto clip with no notes left is removed",
        command {
            track: Id,
            note: Id,
        }
        action {
            track: Id,
            note: Id,
        }
        commit {
            Action::DeleteNote { track, note }
        }
        edit |project: &Project| {
            project.delete_note(*track, *note).map_err(Into::into)
        }
    }
    ResizeClip {
        id: "clip.resize",
        name_ja: "クリップの範囲を変える",
        name_en: "Resize a clip",
        description_ja: "範囲を変え、クリップを固定する。ノートの時刻は動かない",
        description_en: "Change the range and fix the clip. Notes stay at the same time",
        command {
            track: Id,
            clip: Id,
            start: Tick,
            length: Tick,
        }
        action {
            track: Id,
            clip: Id,
            start: Tick,
            length: Tick,
        }
        commit {
            Action::ResizeClip {
                track,
                clip,
                start,
                length,
            }
        }
        edit |project: &Project| {
            project
                .resize_clip(*track, *clip, *start, *length)
                .map_err(Into::into)
        }
    }
    AddChild {
        id: "track.add_child",
        name_ja: "子トラックを追加",
        name_en: "Add a child track",
        description_ja: "フォルダの中にトラックを置く。深さに上限はない",
        description_en: "Place a track inside a folder. Nesting has no fixed depth",
        command {
            parent: Id,
            name: String,
        }
        action {
            id: Id,
            parent: Id,
            name: String,
            device: Id,
            gain: Id,
            pan: Id,
            mute: Id,
            solo: Id,
        }
        commit {
            let mixer = MixerIds::generate();
            Action::AddChild {
                id: Id::generate(),
                parent,
                name,
                device: mixer.device,
                gain: mixer.gain,
                pan: mixer.pan,
                mute: mixer.mute,
                solo: mixer.solo,
            }
        }
        edit |project: &Project| {
            project
                .add_child_with_id(
                    *id,
                    *parent,
                    name.clone(),
                    MixerIds {
                        device: *device,
                        gain: *gain,
                        pan: *pan,
                        mute: *mute,
                        solo: *solo,
                    },
                )
                .map_err(Into::into)
        }
    }
    SetParent {
        id: "track.set_parent",
        name_ja: "フォルダを変える",
        name_en: "Move a track into a folder",
        description_ja: "トラックの親を変える。自分の子孫の中には入れない",
        description_en: "Change a track's parent. A track cannot move into its own subtree",
        command {
            track: Id,
            parent: Option<Id>,
        }
        action {
            track: Id,
            parent: Option<Id>,
        }
        commit {
            Action::SetParent { track, parent }
        }
        edit |project: &Project| {
            project.set_parent(*track, *parent).map_err(Into::into)
        }
    }
    LinkClip {
        id: "clip.link",
        name_ja: "クリップをリンクして複製",
        name_en: "Link a clip",
        description_ja: "同じ中身を指す固定の配置を別の位置に置く",
        description_en: "Place a fixed window onto the same content",
        command {
            source: Id,
            track: Id,
            start: Tick,
        }
        action {
            id: Id,
            source: Id,
            track: Id,
            start: Tick,
        }
        commit {
            Action::LinkClip {
                id: Id::generate(),
                source,
                track,
                start,
            }
        }
        edit |project: &Project| {
            project
                .link_clip(*source, *track, *start, *id)
                .map_err(Into::into)
        }
    }
    SetClipGain {
        id: "clip.set_gain",
        name_ja: "クリップのゲインを変える",
        name_en: "Set clip gain",
        description_ja: "配置の線形ゲインを変える。0 は無音",
        description_en: "Set a placement's linear gain. Zero is silence",
        command {
            clip: Id,
            gain: f32,
        }
        action {
            clip: Id,
            gain_bits: u32,
        }
        commit {
            Action::SetClipGain {
                clip,
                gain_bits: gain.to_bits(),
            }
        }
        edit |project: &Project| {
            project
                .set_clip_gain(*clip, f32::from_bits(*gain_bits))
                .map_err(Into::into)
        }
    }
    SetParameter {
        id: "param.set",
        name_ja: "パラメータを変える",
        name_en: "Set a parameter",
        description_ja: "ミキサーの現在値を変える",
        description_en: "Change a mixer parameter's current value",
        command {
            parameter: Id,
            value: f32,
        }
        action {
            parameter: Id,
            value_bits: u32,
        }
        commit {
            Action::SetParameter {
                parameter,
                value_bits: value.to_bits(),
            }
        }
        edit |project: &Project| {
            project
                .set_parameter(*parameter, f32::from_bits(*value_bits))
                .map_err(Into::into)
        }
    }
    SetClipFades {
        id: "clip.set_fades",
        name_ja: "クリップのフェードを変える",
        name_en: "Set clip fades",
        description_ja: "配置の入りと出のフェードを flick で変える。0 ならかけない",
        description_en: "Set a placement's edge fades in flicks. Zero disables them",
        command {
            clip: Id,
            fade_in: i64,
            fade_out: i64,
        }
        action {
            clip: Id,
            fade_in: i64,
            fade_out: i64,
        }
        commit {
            Action::SetClipFades {
                clip,
                fade_in,
                fade_out,
            }
        }
        edit |project: &Project| {
            project
                .set_clip_fades(*clip, *fade_in, *fade_out)
                .map_err(Into::into)
        }
    }
    AddDevice {
        id: "device.add",
        name_ja: "プラグインを挿入",
        name_en: "Insert a plugin",
        description_ja: "CLAP の ID でデバイスをミキサーの前に置く",
        description_en: "Insert a CLAP device, identified by its plugin id, before the mixer",
        command {
            track: Id,
            plugin: String,
        }
        action {
            track: Id,
            device: Id,
            plugin: String,
        }
        commit {
            Action::AddDevice {
                track,
                device: Id::generate(),
                plugin,
            }
        }
        edit |project: &Project| {
            project
                .add_clap_device(*track, *device, plugin.clone())
                .map_err(Into::into)
        }
    }
    RemoveDevice {
        id: "device.remove",
        name_ja: "プラグインを外す",
        name_en: "Remove a plugin",
        description_ja: "挿入した CLAP デバイスを外す。ミキサーは残る",
        description_en: "Remove an inserted CLAP device. The mixer stays",
        command {
            device: Id,
        }
        action {
            device: Id,
        }
        commit {
            Action::RemoveDevice { device }
        }
        edit |project: &Project| {
            project.remove_device(*device).map_err(Into::into)
        }
    }
    Restore {
        id: "history.restore",
        name_ja: "履歴から戻す",
        name_en: "Restore a saved revision",
        description_ja: "自動保存した版を新しい版として戻す。それまでのコミットは残る",
        description_en: "Restore an autosaved revision as a new version. Earlier commits stay",
        command {
            revision: String,
        }
        action {
            revision: String,
        }
        commit {
            Action::Restore { revision }
        }
        edit |_project: &Project| {
            let _ = revision;
            Err(ActionError::NotApplied)
        }
    }
    PlaceAudio {
        id: "audio.place",
        name_ja: "音声を置く",
        name_en: "Place audio",
        description_ja: "保管済みの音声を固定クリップとして置く",
        description_en: "Place stored audio as a fixed clip",
        command {
            track: Id,
            start: Tick,
            hash: String,
            sample_rate: u32,
            channels: u16,
            frames: u64,
        }
        action {
            track: Id,
            clip: Id,
            content: Id,
            start: Tick,
            hash: String,
            sample_rate: u32,
            channels: u16,
            frames: u64,
        }
        commit {
            Action::PlaceAudio {
                track,
                clip: Id::generate(),
                content: Id::generate(),
                start,
                hash,
                sample_rate,
                channels,
                frames,
            }
        }
        edit |project: &Project| {
            let audio = AudioRef::try_new(hash.clone(), *sample_rate, *channels, *frames)?;
            project
                .place_audio(*track, *clip, *content, *start, audio)
                .map_err(Into::into)
        }
    }
    ImportAudio {
        id: "audio.import",
        name_ja: "音声を取り込む",
        name_en: "Import audio",
        description_ja: "音声ファイルを保管庫に入れて、トラックの固定クリップにする",
        description_en: "Store an audio file and place it as a fixed clip",
        command {
            track: Id,
            start: Tick,
            path: String,
        }
        action {
            track: Id,
            start: Tick,
            path: String,
        }
        commit {
            Action::ImportAudio { track, start, path }
        }
        edit |_project: &Project| {
            let _ = (track, start, path);
            Err(ActionError::NotApplied)
        }
    }
}

impl Action {
    pub(crate) fn apply(&self, project: &Project) -> Result<(Project, ChangeSet), ActionError> {
        let next = self.edit(project)?;
        let changes = project.diff(&next);
        debug_assert_eq!(project.apply_changeset(&changes).ok().as_ref(), Some(&next));
        next.debug_assert_invariants();
        Ok((next, changes))
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
    /// The document host applies this command after it loads a file or a revision.
    NotApplied,
}

impl fmt::Display for ActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(error) => write!(formatter, "{error}"),
            Self::NothingToUndo => formatter.write_str("nothing to undo"),
            Self::NothingToRedo => formatter.write_str("nothing to redo"),
            Self::CorruptJournal => formatter.write_str("journal entry is not valid"),
            Self::NotApplied => formatter.write_str("a saved revision is restored by the document"),
        }
    }
}

impl std::error::Error for ActionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Model(error) => Some(error),
            Self::NothingToUndo | Self::NothingToRedo | Self::CorruptJournal | Self::NotApplied => {
                None
            }
        }
    }
}

/// The sentence a client sees when a command does not deserialize.
pub fn parameter_message(error: &serde_json::Error) -> String {
    let message = error.to_string();
    for sentence in [
        "meter does not divide into ticks",
        "tempo is outside the allowed range",
    ] {
        if message.contains(sentence) {
            return sentence.to_string();
        }
    }
    if let Some(start) = message.find("invalid id ") {
        let rest = &message[start..];
        let end = rest.find(" at line").unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    "parameters do not match an action".to_string()
}
