//! Object key (S3 object key) newtype.

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// A validated object key (S3 object key).
///
/// Validation:
/// - max 1024 bytes
/// - must not be empty
/// - must not start with `/`
/// - must not contain `..` path traversal segments
/// - must be valid UTF-8 (enforced by `&str` input)
/// - must not contain `\r` or `\n`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct ObjectKey(String);

impl ObjectKey {
    /// Parse and validate an object key.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidObjectKey`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_object_key(s)
    }

    /// Create from an owned string.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidObjectKey`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        validate_object_key(&s)
    }

    /// Return as string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume and return inner string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

fn validate_object_key(input: &str) -> Result<ObjectKey, ValidError> {
    if input.is_empty() {
        return Err(ValidError::InvalidObjectKey(
            "object key is empty".to_string(),
        ));
    }
    if input.len() > 1024 {
        return Err(ValidError::InvalidObjectKey(alloc::format!(
            "object key exceeds 1024 bytes, got {}",
            input.len()
        )));
    }
    if input.starts_with('/') {
        return Err(ValidError::InvalidObjectKey(
            "object key must not start with '/'".to_string(),
        ));
    }
    if input.contains('\r') || input.contains('\n') {
        return Err(ValidError::InvalidObjectKey(
            "object key contains CR or LF".to_string(),
        ));
    }
    // Check for path traversal: any segment equals ".."
    for segment in input.split('/') {
        if segment == ".." {
            return Err(ValidError::InvalidObjectKey(
                "object key must not contain '..' traversal".to_string(),
            ));
        }
        // Also disallow empty segment at start? Already handled leading slash.
        // Allow consecutive slashes? AWS allows them but we treat as suspicious?
        // We allow `//` but not `..`.
    }
    // Also disallow literal ".." anywhere (even without slashes) to be safe, but spec says no .. traversal.
    // If input contains ".." as substring not as segment, is it traversal? e.g., "file..txt" should be allowed.
    // So only segment check above is needed. Remove broad check.
    // For safety, also reject null byte.
    if input.contains('\0') {
        return Err(ValidError::InvalidObjectKey(
            "object key must not contain null byte".to_string(),
        ));
    }
    Ok(ObjectKey(input.to_string()))
}

/// Returns `true` if `s` is a valid object key.
#[must_use]
pub fn is_valid_object_key(s: &str) -> bool {
    validate_object_key(s).is_ok()
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ObjectKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for ObjectKey {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ObjectKey {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        ObjectKey::new(value)
    }
}

impl TryFrom<&str> for ObjectKey {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        ObjectKey::parse(value)
    }
}

impl FromStr for ObjectKey {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ObjectKey::parse(s)
    }
}

impl AsRef<str> for ObjectKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

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
    fn valid_keys() {
        assert!(ObjectKey::parse("foo/bar/baz.txt").is_ok());
        assert!(ObjectKey::parse("file..txt").is_ok());
        assert!(ObjectKey::parse("a").is_ok());
        assert!(ObjectKey::parse("foo/bar//baz").is_ok());
    }

    #[test]
    fn invalid_traversal() {
        assert!(ObjectKey::parse("../etc/passwd").is_err());
        assert!(ObjectKey::parse("foo/../bar").is_err());
        assert!(ObjectKey::parse("foo/..").is_err());
    }

    #[test]
    fn invalid_leading_slash() {
        assert!(ObjectKey::parse("/foo/bar").is_err());
    }

    #[test]
    fn invalid_too_long() {
        let long = "a".repeat(1025);
        assert!(ObjectKey::parse(&long).is_err());
    }
}
