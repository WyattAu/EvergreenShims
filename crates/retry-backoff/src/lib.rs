#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Generic async retry with exponential backoff and jitter.
//!
//! Use [`with_backoff`] to retry fallible async operations with configurable
//! backoff strategy. Implement [`IsRetryable`] on your error type to control
//! which errors trigger retries.

mod config;
mod retry;
mod traits;

pub use config::RetryConfig;
pub use retry::{RetryError, with_backoff};
pub use traits::IsRetryable;
