//! Error types for blobkit.

use core::fmt;

/// Result alias for blob operations.
pub type Result<T> = core::result::Result<T, BlobError>;

/// Errors that can occur during blob storage operations.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    /// An I/O error occurred.
    #[cfg(feature = "std")]
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// An I/O error occurred (no_std fallback stores message).
    #[cfg(not(feature = "std"))]
    #[error("io error: {0}")]
    Io(alloc::string::String),

    /// The requested key was not found.
    #[error("not found: {0}")]
    NotFound(alloc::string::String),

    /// A key already exists and the operation requires absence.
    #[error("already exists")]
    AlreadyExists,

    /// The supplied key or bucket name is invalid.
    #[error("invalid key: {0}")]
    InvalidKey(alloc::string::String),

    /// Storage capacity has been exhausted.
    #[error("storage full")]
    StorageFull,

    /// The caller lacks permission for the operation.
    #[error("permission denied: {0}")]
    PermissionDenied(alloc::string::String),

    /// The operation is not supported by this backend.
    #[error("unsupported: {0}")]
    Unsupported(alloc::string::String),

    /// A generic storage error.
    #[error("storage error: {0}")]
    Other(alloc::string::String),
}

impl BlobError {
    /// Create an `InvalidKey` error from any displayable value.
    pub fn invalid_key(msg: impl fmt::Display) -> Self {
        Self::InvalidKey(alloc::string::ToString::to_string(&msg))
    }

    /// Create a `NotFound` error.
    pub fn not_found(msg: impl fmt::Display) -> Self {
        Self::NotFound(alloc::string::ToString::to_string(&msg))
    }

    /// Create an `Unsupported` error.
    pub fn unsupported(msg: impl fmt::Display) -> Self {
        Self::Unsupported(alloc::string::ToString::to_string(&msg))
    }

    /// Create a `PermissionDenied` error.
    pub fn permission_denied(msg: impl fmt::Display) -> Self {
        Self::PermissionDenied(alloc::string::ToString::to_string(&msg))
    }

    /// Returns `true` if the error is `NotFound`.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }

    /// Returns `true` if the error is `InvalidKey`.
    #[must_use]
    pub fn is_invalid_key(&self) -> bool {
        matches!(self, Self::InvalidKey(_))
    }
}

// Enable conversion from tempfile errors when std is available.
#[cfg(feature = "std")]
impl From<tempfile::PersistError> for BlobError {
    fn from(e: tempfile::PersistError) -> Self {
        Self::Io(e.error)
    }
}
