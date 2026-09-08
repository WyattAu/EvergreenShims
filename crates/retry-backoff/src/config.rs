//! Retry configuration.

use std::time::Duration;

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries, 1 = one retry).
    pub max_retries: u32,

    /// Initial delay before the first retry.
    pub initial_delay: Duration,

    /// Maximum delay between retries (caps exponential growth).
    pub max_delay: Duration,

    /// Multiplier for exponential backoff (e.g., 2.0 doubles each time).
    pub backoff_multiplier: f64,

    /// Whether to add random jitter to the delay.
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryConfig {
    /// Create a config with no retries (try once only).
    pub fn no_retries() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Create a config for aggressive retries (10 attempts, fast backoff).
    pub fn aggressive() -> Self {
        Self {
            max_retries: 10,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }

    /// Calculate the delay for a given attempt number (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base = self.initial_delay.as_millis() as f64;
        let delay_ms = base * self.backoff_multiplier.powi(attempt as i32);
        let capped_ms = delay_ms.min(self.max_delay.as_millis() as f64);

        if self.jitter {
            let jitter_range = capped_ms * 0.1; // 10% jitter
            let jitter = rand::random::<f64>() * jitter_range;
            Duration::from_millis((capped_ms + jitter) as u64)
        } else {
            Duration::from_millis(capped_ms as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let c = RetryConfig::default();
        assert_eq!(c.max_retries, 3);
        assert_eq!(c.initial_delay, Duration::from_millis(100));
    }

    #[test]
    fn delay_increases_exponentially() {
        let c = RetryConfig {
            jitter: false,
            ..Default::default()
        };
        let d0 = c.delay_for_attempt(0);
        let d1 = c.delay_for_attempt(1);
        let d2 = c.delay_for_attempt(2);
        assert!(d1 > d0);
        assert!(d2 > d1);
    }

    #[test]
    fn delay_respects_max() {
        let c = RetryConfig {
            max_delay: Duration::from_millis(200),
            jitter: false,
            ..Default::default()
        };
        let d = c.delay_for_attempt(100);
        assert!(d <= Duration::from_millis(200));
    }
}
