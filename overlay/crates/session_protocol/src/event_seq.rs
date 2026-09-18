//! Monotonic sequence numbers for the server-side agent event log.
//!
//! Clients store the last observed seq and call catch-up with it after a
//! reconnect. Disconnect does not rewind or drop the log.

use std::fmt;

/// Append-only log position. Never wraps: saturates at `u64::MAX`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventSeq(pub u64);

impl EventSeq {
    pub const ZERO: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    pub fn saturating_add(self, delta: u64) -> Self {
        Self(self.0.saturating_add(delta))
    }
}

impl fmt::Display for EventSeq {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_monotonically() {
        let first = EventSeq::ZERO.next();
        let second = first.next();
        assert!(first < second);
        assert_eq!(first.0, 1);
        assert_eq!(second.0, 2);
    }

    #[test]
    fn saturates_instead_of_wrapping() {
        let max = EventSeq(u64::MAX);
        assert_eq!(max.next(), max);
    }
}
