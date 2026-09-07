// vendored: allow pre-existing lints (upstream repos use different lint config)
#![allow(clippy::indexing_slicing, clippy::needless_return)]
//! Typed primitives for blob storage.
//!
//! All key types perform validation at construction time so invalid values
//! are rejected before any I/O is attempted.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use core::fmt;
use core::str::FromStr;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "chrono")]
use chrono::{DateTime, Utc};

#[cfg(feature = "typed-id")]
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ObjectKey
// ---------------------------------------------------------------------------

/// Validated object key.
///
/// Constraints:
/// - non-empty
/// - length `<= 1024` bytes
/// - must not contain `..` as a path component
/// - must not contain empty segments (`//`) or leading/trailing slashes that
///   would create empty components (a single leading slash is rejected)
/// - must not contain null bytes or control characters (`\0`, `\r`, `\n`)
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct ObjectKey(String);

impl ObjectKey {
    /// Maximum byte length for a key.
    pub const MAX_LEN: usize = 1024;

    /// Validate and construct an `ObjectKey`.
    ///
    /// # Errors
    /// Returns [`crate::error::BlobError::InvalidKey`] if validation fails.
    ///
    /// # Requirements
    /// REQ-BK-100, REQ-BK-101
    pub fn new(key: impl Into<String>) -> Result<Self, crate::error::BlobError> {
        let s = key.into();
        Self::validate(&s)?;
        Ok(Self(s))
    }

    /// Construct without validation. Used internally when the key has already
    /// been validated or comes from a trusted source (e.g. storage listing).
    #[must_use]
    pub fn new_unchecked(s: String) -> Self {
        Self(s)
    }

    /// Borrow the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into the inner `String`.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    fn validate(s: &str) -> Result<(), crate::error::BlobError> {
        if s.is_empty() {
            return Err(crate::error::BlobError::invalid_key(
                "key must not be empty",
            ));
        }
        if s.len() > Self::MAX_LEN {
            return Err(crate::error::BlobError::invalid_key(format!(
                "key length {} exceeds maximum {}",
                s.len(),
                Self::MAX_LEN
            )));
        }
        if s.contains('\0') || s.contains('\r') || s.contains('\n') {
            return Err(crate::error::BlobError::invalid_key(
                "key must not contain null or control characters",
            ));
        }
        // Reject any ".." path component — prevents directory traversal.
        for component in s.split('/') {
            if component == ".." {
                return Err(crate::error::BlobError::invalid_key(
                    "key must not contain '..' path component",
                ));
            }
        }
        // Reject empty segments (e.g. "a//b" or "/leading" or "trailing/")
        // The empty split result from leading/trailing slashes produces an empty
        // string component which we treat as invalid except for the trivial case
        // where the key itself is validated as non-empty; leading slash yields
        // first component == "" which is considered invalid.
        if s.starts_with('/') || s.ends_with('/') || s.contains("//") {
            return Err(crate::error::BlobError::invalid_key(
                "key must not have empty path segments or leading/trailing slashes",
            ));
        }
        Ok(())
    }
}

impl fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ObjectKey {
    type Err = crate::error::BlobError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_string())
    }
}

impl TryFrom<String> for ObjectKey {
    type Error = crate::error::BlobError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for ObjectKey {
    type Error = crate::error::BlobError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s.to_string())
    }
}

impl AsRef<str> for ObjectKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// BucketName
// ---------------------------------------------------------------------------

/// Validated S3 bucket name.
///
/// Constraints (subset of AWS rules):
/// - length `3..=63`
/// - lower-case alphanumeric and hyphen only
/// - must start and end with alphanumeric
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct BucketName(String);

impl BucketName {
    /// Validate and construct.
    ///
    /// # Requirements
    /// REQ-BK-102
    pub fn new(name: impl Into<String>) -> Result<Self, crate::error::BlobError> {
        let s = name.into();
        Self::validate(&s)?;
        Ok(Self(s))
    }

    /// Borrow the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into `String`.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    fn validate(s: &str) -> Result<(), crate::error::BlobError> {
        let len = s.len();
        if !(3..=63).contains(&len) {
            return Err(crate::error::BlobError::invalid_key(format!(
                "bucket name length {len} must be between 3 and 63"
            )));
        }
        let bytes = s.as_bytes();
        // len >= 3 is guaranteed by the check above, so first/last exist.
        if bytes.first().is_some_and(|b| !b.is_ascii_alphanumeric())
            || bytes.last().is_some_and(|b| !b.is_ascii_alphanumeric())
        {
            return Err(crate::error::BlobError::invalid_key(
                "bucket name must start and end with alphanumeric",
            ));
        }
        for &b in bytes {
            let ok = b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
            if !ok {
                return Err(crate::error::BlobError::invalid_key(
                    "bucket name must contain only lowercase alphanumeric and hyphen",
                ));
            }
        }
        // Reject consecutive hyphens? Not needed for minimal validation.
        Ok(())
    }
}

impl fmt::Display for BucketName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BucketName {
    type Err = crate::error::BlobError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_string())
    }
}

impl AsRef<str> for BucketName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// BlobId
// ---------------------------------------------------------------------------

/// Opaque identifier for a stored blob.
///
/// Wraps a UUID v4. Requires the `typed-id` feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct BlobId(
    #[cfg(feature = "typed-id")] pub Uuid,
    #[cfg(not(feature = "typed-id"))] pub [u8; 16],
);

impl BlobId {
    /// Generate a new random `BlobId`.
    #[cfg(feature = "typed-id")]
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Generate a new random `BlobId` (fallback when `typed-id` is disabled).
    #[cfg(all(not(feature = "typed-id"), feature = "std"))]
    #[must_use]
    pub fn new() -> Self {
        // Simple pseudo-random using std::collections::hash_map::DefaultHasher is not
        // cryptographically random but avoids requiring uuid when the feature is off.
        // Prefer enabling `typed-id` in production.
        use core::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // Use time + thread id as entropy source.
        std::time::SystemTime::now().hash(&mut hasher);
        std::thread::current().id().hash(&mut hasher);
        let h = hasher.finish();
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&h.to_le_bytes());
        bytes[8..].copy_from_slice(&(!h).to_le_bytes());
        Self(bytes)
    }

    /// Generate a new `BlobId` for `no_std` without `typed-id`.
    #[cfg(all(not(feature = "typed-id"), not(feature = "std")))]
    #[must_use]
    pub fn new() -> Self {
        // Deterministic fallback: incrementing counter would require
        // synchronization. For no_std we return a fixed value; callers
        // requiring uniqueness should enable `typed-id`.
        Self([0xAB; 16])
    }

    /// Create from a UUID.
    #[cfg(feature = "typed-id")]
    #[must_use]
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// Borrow the inner UUID.
    #[cfg(feature = "typed-id")]
    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

#[cfg(feature = "typed-id")]
impl Default for BlobId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(feature = "typed-id"))]
impl Default for BlobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(feature = "typed-id")]
        return write!(f, "{}", self.0);
        #[cfg(not(feature = "typed-id"))]
        {
            for b in &self.0 {
                write!(f, "{b:02x}")?;
            }
            Ok(())
        }
    }
}

impl FromStr for BlobId {
    type Err = crate::error::BlobError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        #[cfg(feature = "typed-id")]
        {
            let uuid = Uuid::parse_str(s).map_err(|e| {
                crate::error::BlobError::invalid_key(format!("invalid BlobId: {e}"))
            })?;
            Ok(Self(uuid))
        }
        #[cfg(not(feature = "typed-id"))]
        {
            if s.len() != 32 {
                return Err(crate::error::BlobError::invalid_key(
                    "invalid BlobId length",
                ));
            }
            let mut bytes = [0u8; 16];
            for i in 0..16 {
                let hex = &s[i * 2..i * 2 + 2];
                bytes[i] = u8::from_str_radix(hex, 16)
                    .map_err(|_| crate::error::BlobError::invalid_key("invalid hex in BlobId"))?;
            }
            Ok(Self(bytes))
        }
    }
}

// ---------------------------------------------------------------------------
// BlobMetadata
// ---------------------------------------------------------------------------

/// Metadata associated with a stored blob.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BlobMetadata {
    /// The object key this metadata refers to.
    pub key: ObjectKey,
    /// Size in bytes.
    pub size: u64,
    /// MIME content type (e.g. `"application/octet-stream"`).
    pub content_type: String,
    /// Creation timestamp, if the `chrono` feature is enabled and a clock is
    /// available. Stored as RFC3339 when serialized.
    #[cfg(feature = "chrono")]
    pub created_at: DateTime<Utc>,
    /// Creation timestamp placeholder when `chrono` is not enabled.
    #[cfg(not(feature = "chrono"))]
    pub created_at: Option<String>,
    /// Hex-encoded SHA-256 digest of the content, if the `sha2` feature is
    /// enabled and hashing was performed.
    #[cfg(feature = "sha2")]
    pub sha256: Option<String>,
    /// SHA-256 placeholder when `sha2` is disabled.
    #[cfg(not(feature = "sha2"))]
    pub sha256: Option<String>,
}

impl BlobMetadata {
    /// Create new metadata with sensible defaults.
    #[must_use]
    pub fn new(key: ObjectKey, size: u64) -> Self {
        Self {
            key,
            size,
            content_type: "application/octet-stream".to_string(),
            #[cfg(feature = "chrono")]
            created_at: Utc::now(),
            #[cfg(not(feature = "chrono"))]
            created_at: None,
            sha256: None,
        }
    }

    /// Create metadata with an explicit content type.
    #[must_use]
    pub fn with_content_type(mut self, ct: impl Into<String>) -> Self {
        self.content_type = ct.into();
        self
    }

    /// Attach a SHA-256 digest.
    #[cfg(feature = "sha2")]
    #[must_use]
    pub fn with_sha256(mut self, digest: impl Into<String>) -> Self {
        self.sha256 = Some(digest.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Guess a content type from a key's extension.
///
/// Uses `mime_guess` when the feature is not available it returns
/// `application/octet-stream`.
#[must_use]
pub fn guess_content_type(key: &ObjectKey) -> String {
    #[cfg(feature = "mime_guess")]
    {
        let guess = mime_guess::from_path(key.as_str()).first_raw();
        guess.unwrap_or("application/octet-stream").to_string()
    }
    #[cfg(not(feature = "mime_guess"))]
    {
        let _ = key;
        "application/octet-stream".to_string()
    }
}

/// Compute hex-encoded SHA-256 of `data` when the `sha2` feature is enabled.
///
/// # Requirements
/// REQ-BK-005
#[cfg(feature = "sha2")]
#[must_use]
pub fn compute_sha256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in result {
        use core::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Compute stub when `sha2` is disabled (returns `None`).
#[cfg(not(feature = "sha2"))]
#[must_use]
pub fn compute_sha256(_data: &[u8]) -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// Tests (unit)
// ---------------------------------------------------------------------------

// Tests exercise failure paths and invariants directly; unwrap/expect,
// slicing, and panicking asserts are acceptable here — violations
// surface as test failures, not production panics.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_key_valid() {
        assert!(ObjectKey::new("hello/world.txt").is_ok());
        assert!(ObjectKey::new("a").is_ok());
        assert!(ObjectKey::new("foo/bar/baz").is_ok());
        assert_eq!(ObjectKey::new("a/b").unwrap().as_str(), "a/b");
    }

    #[test]
    fn object_key_rejects_empty() {
        assert!(ObjectKey::new("").is_err());
    }

    #[test]
    fn object_key_rejects_dotdot() {
        assert!(ObjectKey::new("../etc/passwd").is_err());
        assert!(ObjectKey::new("a/../b").is_err());
        assert!(ObjectKey::new("a/b/..").is_err());
        // "..hello" is fine — only isolated ".." component is rejected
        assert!(ObjectKey::new("..hello").is_ok());
        assert!(ObjectKey::new("a/..hello/b").is_ok());
    }

    #[test]
    fn object_key_rejects_empty_segments() {
        assert!(ObjectKey::new("/leading").is_err());
        assert!(ObjectKey::new("trailing/").is_err());
        assert!(ObjectKey::new("a//b").is_err());
    }

    #[test]
    fn object_key_rejects_too_long() {
        let long = "a".repeat(1025);
        assert!(ObjectKey::new(long).is_err());
        let ok = "a".repeat(1024);
        assert!(ObjectKey::new(ok).is_ok());
    }

    #[test]
    fn object_key_rejects_control_chars() {
        assert!(ObjectKey::new("a\0b").is_err());
        assert!(ObjectKey::new("a\nb").is_err());
        assert!(ObjectKey::new("a\rb").is_err());
    }

    #[test]
    fn bucket_name_valid() {
        assert!(BucketName::new("abc").is_ok());
        assert!(BucketName::new("my-bucket").is_ok());
        assert!(BucketName::new("my-bucket-123").is_ok());
        assert_eq!(BucketName::new("ab1").unwrap().as_str(), "ab1");
    }

    #[test]
    fn bucket_name_invalid() {
        assert!(BucketName::new("ab").is_err()); // too short
        assert!(BucketName::new("a".repeat(64)).is_err()); // too long
        assert!(BucketName::new("MyBucket").is_err()); // uppercase
        assert!(BucketName::new("-abc").is_err()); // leading hyphen
        assert!(BucketName::new("abc-").is_err()); // trailing hyphen
        assert!(BucketName::new("ab_c").is_err()); // underscore
        assert!(BucketName::new("ab.c").is_err()); // dot not allowed in minimal impl
    }

    #[test]
    fn blob_id_roundtrip_display_from_str() {
        let id = BlobId::new();
        let s = id.to_string();
        let parsed = BlobId::from_str(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn blob_metadata_new() {
        let key = ObjectKey::new("file.txt").unwrap();
        let meta = BlobMetadata::new(key.clone(), 42);
        assert_eq!(meta.key, key);
        assert_eq!(meta.size, 42);
        assert_eq!(meta.content_type, "application/octet-stream");
    }

    #[test]
    fn guess_content_type_default() {
        let key = ObjectKey::new("file.bin").unwrap();
        let ct = guess_content_type(&key);
        assert!(!ct.is_empty());
    }
}
