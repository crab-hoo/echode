//! Main-thread relay metadata. No cache borrow crosses a plugin call.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

type Flags = Rc<HashMap<u32, u32>>;

pub(crate) struct ParamFlags {
    generation: Cell<u64>,
    flags: RefCell<Option<Flags>>,
}

impl ParamFlags {
    pub(crate) const fn new() -> Self {
        Self {
            generation: Cell::new(0),
            flags: RefCell::new(None),
        }
    }

    pub(crate) fn invalidate(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.flags.borrow_mut().take();
    }

    pub(crate) fn read(&self, load: impl FnOnce() -> HashMap<u32, u32>) -> Flags {
        if let Some(flags) = self.flags.borrow().as_ref() {
            return Rc::clone(flags);
        }
        let generation = self.generation.get();
        // The plugin can synchronously invalidate this cache from get_info.
        let flags = Rc::new(load());
        if self.generation.get() == generation {
            self.flags.replace(Some(Rc::clone(&flags)));
        }
        flags
    }
}

#[cfg(test)]
#[path = "param_flags_tests.rs"]
mod tests;
