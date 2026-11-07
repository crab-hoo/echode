//! Match sorted content identities once and reuse only the non-shared sides.

use std::cmp::Ordering;
use std::sync::Arc;

use super::content::{content_change, diff_content_fields};
use super::{Change, Content, Project};

pub(super) struct ChangedContents<'a> {
    pub before: Vec<&'a Content>,
    pub after: Vec<&'a Content>,
    removed: Vec<&'a Content>,
}

impl<'a> ChangedContents<'a> {
    pub fn between(
        before: &'a Project,
        after: &'a Project,
        creates: &mut Vec<Change>,
        updates: &mut Vec<Change>,
    ) -> Self {
        let mut changed = Self {
            before: Vec::new(),
            after: Vec::new(),
            removed: Vec::new(),
        };
        let mut previous = contents(before).peekable();
        let mut next = contents(after).peekable();
        // Both snapshots keep contents in ID order. The separate output buffers
        // preserve shell and note ordering, including cross-content note moves.
        loop {
            match (previous.peek(), next.peek()) {
                (Some(left), Some(right)) => match left.id.cmp(&right.id) {
                    Ordering::Less => {
                        changed.before.push(left);
                        changed.removed.push(left);
                        previous.next();
                    }
                    Ordering::Greater => {
                        creates.push(content_change(true, right));
                        changed.after.push(right);
                        next.next();
                    }
                    Ordering::Equal => {
                        if !Arc::ptr_eq(left, right) {
                            diff_content_fields(left, right, updates);
                            changed.before.push(left);
                            changed.after.push(right);
                        }
                        previous.next();
                        next.next();
                    }
                },
                (Some(left), None) => {
                    changed.before.push(left);
                    changed.removed.push(left);
                    previous.next();
                }
                (None, Some(right)) => {
                    creates.push(content_change(true, right));
                    changed.after.push(right);
                    next.next();
                }
                (None, None) => break,
            }
        }
        changed
    }

    pub fn push_deletes(&self, deletes: &mut Vec<Change>) {
        deletes.extend(
            self.removed
                .iter()
                .map(|content| content_change(false, content)),
        );
    }
}

fn contents(project: &Project) -> impl Iterator<Item = &Arc<Content>> {
    project.contents().iter().inspect(|_| {
        #[cfg(test)]
        crate::model_scale_tests::content_key();
    })
}
