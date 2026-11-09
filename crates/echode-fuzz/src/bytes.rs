//! Reads small numbers from the front of a fuzz input. A short input reads as
//! zeros, so every prefix of an input is still an input.

pub(crate) struct Bytes<'a> {
    rest: &'a [u8],
}

impl<'a> Bytes<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { rest: data }
    }

    pub(crate) fn u8(&mut self) -> u8 {
        match self.rest.split_first() {
            Some((&byte, rest)) => {
                self.rest = rest;
                byte
            }
            None => 0,
        }
    }

    pub(crate) fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }

    /// Up to `len` bytes, fewer when the input ends first.
    pub(crate) fn take(&mut self, len: usize) -> &'a [u8] {
        let (taken, rest) = self.rest.split_at(len.min(self.rest.len()));
        self.rest = rest;
        taken
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }
}
