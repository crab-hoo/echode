//! Which tracks feed which bus, and when each one may start in a block.
//!
//! A child's post-fader audio enters its parent's source before the parent's
//! chain starts (ADR 0277, ADR 0338). Children finish in any order. A parent
//! starts when its last child finishes, so a late helper delays only its own
//! ancestors. Each bus sums its children in plan order, so the result does
//! not depend on which helper answered first.
//!
//! Control sizes everything. The callback only counts, pushes within the
//! reserved capacity and pops; it never allocates or walks the whole tree.

pub(super) struct Bus {
    parents: Vec<Option<usize>>,
    first_child: Vec<Option<usize>>,
    next_sibling: Vec<Option<usize>>,
    first_root: Option<usize>,
    children: Vec<u32>,
    /// Children of each track still running in this block.
    waiting: Vec<u32>,
    /// Tracks whose children have all finished, not yet started.
    ready: Vec<usize>,
    /// Keys of hosted tracks that finished, filled by `HostGraph::progress`.
    pub(super) finished: Vec<u32>,
    started: usize,
    overflow: bool,
}

/// Tracks linked by `next_sibling`, in plan order.
pub(super) struct Siblings<'a> {
    next: Option<usize>,
    links: &'a [Option<usize>],
}

impl Iterator for Siblings<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let current = self.next?;
        self.next = self.links.get(current).copied().flatten();
        Some(current)
    }
}

impl Bus {
    /// `parents` is in plan order, where every parent precedes its children.
    pub(super) fn new(parents: Vec<Option<usize>>) -> Self {
        let count = parents.len();
        let mut first_child = vec![None; count];
        let mut next_sibling = vec![None; count];
        let mut children = vec![0u32; count];
        let mut first_root = None;
        // Backwards, so every list comes out in plan order.
        for (position, parent) in parents.iter().enumerate().rev() {
            let head = match *parent {
                Some(parent) => {
                    if let Some(count) = children.get_mut(parent) {
                        *count = count.saturating_add(1);
                    }
                    first_child.get_mut(parent)
                }
                None => Some(&mut first_root),
            };
            if let Some(head) = head {
                if let Some(next) = next_sibling.get_mut(position) {
                    *next = *head;
                }
                *head = Some(position);
            }
        }
        Self {
            waiting: children.clone(),
            ready: Vec::with_capacity(count),
            finished: Vec::with_capacity(count),
            parents,
            first_child,
            next_sibling,
            first_root,
            children,
            started: 0,
            overflow: false,
        }
    }

    /// Each track's parent position, in plan order.
    pub(super) fn parents(&self) -> &[Option<usize>] {
        &self.parents
    }

    pub(super) fn has_children(&self, position: usize) -> bool {
        self.children.get(position).is_some_and(|count| *count != 0)
    }

    pub(super) fn children_of(&self, position: usize) -> Siblings<'_> {
        Siblings {
            next: self.first_child.get(position).copied().flatten(),
            links: &self.next_sibling,
        }
    }

    pub(super) fn roots(&self) -> Siblings<'_> {
        Siblings {
            next: self.first_root,
            links: &self.next_sibling,
        }
    }

    pub(super) fn begin(&mut self) {
        for (waiting, children) in self.waiting.iter_mut().zip(&self.children) {
            *waiting = *children;
        }
        self.ready.clear();
        self.finished.clear();
        self.started = 0;
        self.overflow = false;
    }

    pub(super) fn note_started(&mut self) {
        self.started = self.started.saturating_add(1);
    }

    /// A track has its output for this block. Its parent becomes ready with
    /// the last child.
    pub(super) fn finish(&mut self, position: usize) {
        let Some(Some(parent)) = self.parents.get(position).copied() else {
            return;
        };
        let Some(waiting) = self.waiting.get_mut(parent) else {
            return;
        };
        *waiting = waiting.saturating_sub(1);
        if *waiting != 0 {
            return;
        }
        // Each parent is pushed once per block, and `ready` holds one place
        // per track, so this never grows the vector.
        if self.ready.len() < self.ready.capacity() {
            self.ready.push(parent);
        } else {
            self.overflow = true;
        }
    }

    pub(super) fn next_ready(&mut self) -> Option<usize> {
        self.ready.pop()
    }

    /// Every track started once and no bookkeeping was lost.
    pub(super) fn complete(&self) -> bool {
        !self.overflow && self.started == self.parents.len()
    }
}

#[cfg(test)]
#[path = "playback_bus_tests.rs"]
mod tests;
