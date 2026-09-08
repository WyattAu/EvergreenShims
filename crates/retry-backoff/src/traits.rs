//! Trait for classifying retryable errors.

/// Trait for errors that can be classified as retryable.
pub trait IsRetryable {
    /// Returns true if the operation should be retried.
    fn is_retryable(&self) -> bool;
}

// Implement for common error types
impl IsRetryable for std::io::Error {
    fn is_retryable(&self) -> bool {
        matches!(
            self.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::WouldBlock
                | std::io::ErrorKind::BrokenPipe
        )
    }
}

impl IsRetryable for std::net::AddrParseError {
    fn is_retryable(&self) -> bool {
        false
    }
}
