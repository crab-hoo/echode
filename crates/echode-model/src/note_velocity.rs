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
        if velocity > 127 {
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
        let mut contents = self.contents().to_vec();
        let content = contents
            .iter_mut()
            .find(|item| item.id == placement.content)
            .ok_or(ModelError::MissingClip)?;
        let target = Arc::make_mut(content)
            .notes
            .iter_mut()
            .find(|item| item.id == note)
            .ok_or(ModelError::MissingNote)?;
        target.velocity = midi7_to_velocity(velocity);
        Ok(self.with_parts(contents, self.tracks().to_vec()))
    }
}
