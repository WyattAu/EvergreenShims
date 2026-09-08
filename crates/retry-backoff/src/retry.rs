//! Core retry logic with exponential backoff.

use crate::config::RetryConfig;
use crate::traits::IsRetryable;

/// Retry an async operation with exponential backoff.
///
/// Calls `f` repeatedly until it succeeds or the max retries are exhausted.
/// On each retryable error, waits for the calculated backoff delay before retrying.
/// Non-retryable errors are returned immediately.
///
/// # Example
///
/// ```ignore
/// use retry_backoff::{with_backoff, RetryConfig};
///
/// let result = with_backoff(&RetryConfig::default(), || async {
///     client.get("https://api.example.com/data").await
/// }).await;
/// ```
pub async fn with_backoff<F, Fut, T, E>(config: &RetryConfig, mut f: F) -> Result<T, RetryError<E>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: IsRetryable + std::fmt::Display,
{
    let mut last_error = None;

    for attempt in 0..=config.max_retries {
        match f().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !err.is_retryable() || attempt == config.max_retries {
                    #[cfg(feature = "tracing")]
                    tracing::warn!(
                        attempt,
                        max_retries = config.max_retries,
                        error = %err,
                        "all retries exhausted"
                    );
                    return Err(RetryError::FinalError(err, attempt));
                }

                let delay = config.delay_for_attempt(attempt);
                #[cfg(feature = "tracing")]
                tracing::error!(
                    attempt,
                    max_retries = config.max_retries,
                    error = %err,
                    "error encountered, will retry"
                );
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    attempt,
                    max_retries = config.max_retries,
                    delay_ms = delay.as_millis() as u64,
                    error = %err,
                    "retrying after error"
                );
                last_error = Some(err);
                tokio::time::sleep(delay).await;
            }
        }
    }

    #[cfg(feature = "tracing")]
    tracing::warn!(
        max_retries = config.max_retries,
        "all retries exhausted (unreachable path)"
    );
    Err(RetryError::FinalError(
        last_error.expect("loop must have executed at least once"),
        config.max_retries,
    ))
}

/// Error type for retry operations.
#[derive(Debug, thiserror::Error)]
pub enum RetryError<E> {
    /// The final error after all retries exhausted.
    #[error("all {1} retries exhausted: {0}")]
    FinalError(E, u32),
}

impl<E: IsRetryable + std::fmt::Display> RetryError<E> {
    /// Get the inner error.
    pub fn into_inner(self) -> E {
        match self {
            Self::FinalError(e, _) => e,
        }
    }

    /// Get the number of retries attempted.
    pub fn retries(&self) -> u32 {
        match self {
            Self::FinalError(_, r) => *r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    #[derive(Debug)]
    struct TestError(bool);

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "test error (retryable={})", self.0)
        }
    }

    impl IsRetryable for TestError {
        fn is_retryable(&self) -> bool {
            self.0
        }
    }

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let config = RetryConfig::no_retries();
        let result = with_backoff(&config, || async { Ok::<_, TestError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let config = RetryConfig {
            initial_delay: Duration::from_millis(1),
            max_retries: 3,
            jitter: false,
            ..Default::default()
        };

        let result = with_backoff(&config, || {
            let a = attempts_clone.clone();
            async move {
                let count = a.fetch_add(1, Ordering::Relaxed);
                if count < 2 {
                    Err(TestError(true))
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn non_retryable_error_returns_immediately() {
        let config = RetryConfig::aggressive();
        let result = with_backoff(&config, || async { Err::<i32, _>(TestError(false)) }).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            RetryError::FinalError(_, retries) => assert_eq!(retries, 0),
        }
    }

    #[tokio::test]
    async fn max_retries_exhausted() {
        let config = RetryConfig {
            initial_delay: Duration::from_millis(1),
            max_retries: 2,
            jitter: false,
            ..Default::default()
        };

        let result = with_backoff(&config, || async { Err::<i32, _>(TestError(true)) }).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            RetryError::FinalError(_, retries) => assert_eq!(retries, 2),
        }
    }
}
