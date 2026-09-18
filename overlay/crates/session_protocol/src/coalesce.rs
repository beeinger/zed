//! Coalesce token deltas so slow SSH is not one Envelope per model token.

use std::time::Duration;

/// When to flush streamed agent text toward a connected client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoalesceConfig {
    pub max_interval: Duration,
    pub max_characters: usize,
}

impl CoalesceConfig {
    pub const SLOW_LINK: Self = Self {
        max_interval: Duration::from_millis(50),
        max_characters: 64,
    };

    pub fn should_flush(self, waited: Duration, buffered_characters: usize) -> bool {
        buffered_characters > 0
            && (waited >= self.max_interval || buffered_characters >= self.max_characters)
    }
}

impl Default for CoalesceConfig {
    fn default() -> Self {
        Self::SLOW_LINK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flushes_on_time_or_size() {
        let config = CoalesceConfig::SLOW_LINK;
        assert!(!config.should_flush(Duration::from_millis(10), 8));
        assert!(config.should_flush(Duration::from_millis(50), 8));
        assert!(config.should_flush(Duration::from_millis(1), 64));
        assert!(!config.should_flush(Duration::from_millis(500), 0));
    }
}
