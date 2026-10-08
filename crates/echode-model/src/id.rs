//! UUIDv7 identities. Every project object keeps one for its whole life.

use std::fmt;
use uuid::Uuid;

/// Stable identity of a project object.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(u128);

impl Id {
    /// A new time-ordered id.
    pub fn generate() -> Self {
        Self(Uuid::now_v7().as_u128())
    }

    /// Restores an id that was stored earlier.
    pub const fn from_raw(raw: u128) -> Self {
        Self(raw)
    }

    /// Parses the hyphenated text written into the journal.
    pub fn parse(text: &str) -> Option<Self> {
        Uuid::parse_str(text).ok().map(|uuid| Self(uuid.as_u128()))
    }

    pub const fn raw(self) -> u128 {
        self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", Uuid::from_u128(self.0).hyphenated())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Id({self})")
    }
}
