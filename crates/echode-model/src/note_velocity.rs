use std::sync::Arc;

use crate::{Id, ModelError, Project, midi7_to_velocity};

impl Project {
    /// Changes only attack velocity in shared content. Placement windows,
    /// note identity, timing, release velocity, and expressions remain intact.
    pub fn set_note_velocity(
        &self,
        track: Id,
        clip: Id,
        note: Id,
        velocity: u8,
    ) -> Result<Self, ModelError> {
        if velocity > crate::MIDI7_MAX {
            return Err(ModelError::InvalidNote);
        }
        let owner = self
            .tracks()
            .iter()
            .find(|item| item.id == track)
            .ok_or(ModelError::MissingTrack)?;
        let placement = owner
            .clips
            .iter()
            .find(|item| item.id == clip)
            .ok_or(ModelError::MissingClip)?;
        let content_index = crate::model::content_index(self.contents(), placement.content)
            .ok_or(ModelError::MissingClip)?;
        let content = &self.contents()[content_index];
        let note_index = content
            .notes
            .iter()
            .position(|item| item.id == note)
            .ok_or(ModelError::MissingNote)?;
        if content.notes[note_index].velocity == midi7_to_velocity(velocity) {
            return Ok(self.clone());
        }
        let mut contents = self.contents().to_vec();
        let content = &mut contents[content_index];
        #[cfg(test)]
        crate::model_scale_tests::velocity_content_copy(content.notes.len());
        let target = &mut Arc::make_mut(content).notes[note_index];
        target.velocity = midi7_to_velocity(velocity);
        Ok(self.with_parts(contents, self.tracks().to_vec()))
    }
}
