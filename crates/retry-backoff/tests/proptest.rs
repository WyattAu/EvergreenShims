//! Property-based tests for loop-retry crate.

use proptest::prelude::*;
use std::time::Duration;

use loop_retry::RetryConfig;

proptest! {
    #[test]
    fn delay_for_attempt_always_non_negative(
        max_retries in 0u32..100,
        initial_ms in 1u64..10_000,
        max_delay_ms in 1u64..60_000,
        backoff_mult in 1.0f64..10.0,
        attempt in 0u32..200,
    ) {
        let config = RetryConfig {
            max_retries,
            initial_delay: Duration::from_millis(initial_ms),
            max_delay: Duration::from_millis(max_delay_ms),
            backoff_multiplier: backoff_mult,
            jitter: false,
        };
        let delay = config.delay_for_attempt(attempt);
        prop_assert!(delay >= Duration::ZERO,
            "delay must be non-negative, got {:?}", delay);
    }

    #[test]
    fn delay_never_exceeds_max(
        max_retries in 0u32..100,
        initial_ms in 1u64..10_000,
        max_delay_ms in 1u64..60_000,
        backoff_mult in 1.0f64..10.0,
        attempt in 0u32..200,
    ) {
        let config = RetryConfig {
            max_retries,
            initial_delay: Duration::from_millis(initial_ms),
            max_delay: Duration::from_millis(max_delay_ms),
            backoff_multiplier: backoff_mult,
            jitter: false,
        };
        let delay = config.delay_for_attempt(attempt);
        prop_assert!(delay <= Duration::from_millis(max_delay_ms),
            "delay {:?} exceeded max {:?}", delay, max_delay_ms);
    }

    #[test]
    fn delay_increases_monotonically_without_jitter(
        initial_ms in 1u64..1_000,
        max_delay_ms in 10_000u64..60_000,
        backoff_mult in 1.5f64..5.0,
    ) {
        let config = RetryConfig {
            max_retries: 100,
            initial_delay: Duration::from_millis(initial_ms),
            max_delay: Duration::from_millis(max_delay_ms),
            backoff_multiplier: backoff_mult,
            jitter: false,
        };
        let d0 = config.delay_for_attempt(0);
        let d1 = config.delay_for_attempt(1);
        let d2 = config.delay_for_attempt(2);
        prop_assert!(d1 > d0, "delay must increase from attempt 0 to 1");
        prop_assert!(d2 > d1, "delay must increase from attempt 1 to 2");
    }

    #[test]
    fn delay_with_jitter_still_non_negative(
        initial_ms in 1u64..10_000,
        max_delay_ms in 1u64..60_000,
        attempt in 0u32..200,
    ) {
        let config = RetryConfig {
            max_retries: 100,
            initial_delay: Duration::from_millis(initial_ms),
            max_delay: Duration::from_millis(max_delay_ms),
            backoff_multiplier: 2.0,
            jitter: true,
        };
        let delay = config.delay_for_attempt(attempt);
        prop_assert!(delay >= Duration::ZERO,
            "delay with jitter must be non-negative, got {:?}", delay);
    }

    #[test]
    fn no_retries_config_zero_max(initial_ms in 1u64..10_000) {
        let config = RetryConfig {
            max_retries: 0,
            initial_delay: Duration::from_millis(initial_ms),
            ..Default::default()
        };
        prop_assert_eq!(config.max_retries, 0);
    }

    #[test]
    fn config_clone_preserves_values(
        max_retries in 0u32..50,
        initial_ms in 1u64..5_000,
        max_delay_ms in 1_000u64..30_000,
        backoff_mult in 1.0f64..5.0,
        jitter in proptest::bool::ANY,
    ) {
        let config = RetryConfig {
            max_retries,
            initial_delay: Duration::from_millis(initial_ms),
            max_delay: Duration::from_millis(max_delay_ms),
            backoff_multiplier: backoff_mult,
            jitter,
        };
        let cloned = config.clone();
        prop_assert_eq!(config.max_retries, cloned.max_retries);
        prop_assert_eq!(config.initial_delay, cloned.initial_delay);
        prop_assert_eq!(config.max_delay, cloned.max_delay);
        prop_assert_eq!(config.backoff_multiplier, cloned.backoff_multiplier);
        prop_assert_eq!(config.jitter, cloned.jitter);
    }
}
