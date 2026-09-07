//! S3 bucket name newtype.

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// A validated S3 bucket name.
///
/// Rules (AWS S3):
/// - 3–63 characters
/// - lowercase letters, numbers, hyphen, dot
/// - must not be formatted as an IP address (e.g. `192.168.1.1`)
/// - must not start or end with hyphen or dot
/// - must not contain `..` (adjacent dots) is not strictly forbidden by AWS
///   but we disallow for safety? Spec says not IP, not start/end hyphen. We'll
///   also disallow consecutive dots for strictness? Keep simple: allow but
///   check no adjacent dots optionally? We check and reject `..` for safety.
/// - must not contain `..` traversal is irrelevant for bucket but keep.
/// - must not be empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct BucketName(String);

impl BucketName {
    /// Parse and validate a bucket name.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidBucketName`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_bucket(s)
    }

    /// Create from an owned string.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidBucketName`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        validate_bucket(&s)
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

fn is_ip_like(s: &str) -> bool {
    // Check if s looks like IPv4: four dot-separated decimal numbers 0-255.
    let parts: alloc::vec::Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    for p in parts {
        if p.is_empty() || p.len() > 3 {
            return false;
        }
        if !p.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        // Leading zeros allowed? AWS disallows IP-like regardless.
        // Parse numeric value 0-255.
        let val: u16 = match p.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        if val > 255 {
            return false;
        }
    }
    true
}

fn validate_bucket(input: &str) -> Result<BucketName, ValidError> {
    if input.len() < 3 || input.len() > 63 {
        return Err(ValidError::InvalidBucketName(alloc::format!(
            "bucket name must be 3-63 characters, got {}",
            input.len()
        )));
    }
    if input.contains('\r') || input.contains('\n') {
        return Err(ValidError::InvalidBucketName(
            "bucket name contains CR or LF".to_string(),
        ));
    }
    if input.starts_with('-')
        || input.ends_with('-')
        || input.starts_with('.')
        || input.ends_with('.')
    {
        return Err(ValidError::InvalidBucketName(
            "bucket name must not start or end with '-' or '.'".to_string(),
        ));
    }
    if input.contains("..") {
        return Err(ValidError::InvalidBucketName(
            "bucket name must not contain consecutive dots".to_string(),
        ));
    }
    if input.contains(".-") || input.contains("-.") {
        return Err(ValidError::InvalidBucketName(
            "bucket name must not contain '.-' or '-.'".to_string(),
        ));
    }
    for ch in input.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' && ch != '.' {
            return Err(ValidError::InvalidBucketName(alloc::format!(
                "invalid character '{}' in bucket name",
                ch
            )));
        }
    }
    if is_ip_like(input) {
        return Err(ValidError::InvalidBucketName(
            "bucket name must not be formatted as an IP address".to_string(),
        ));
    }
    Ok(BucketName(input.to_string()))
}

/// Returns `true` if `s` is a valid bucket name.
#[must_use]
pub fn is_valid_bucket_name(s: &str) -> bool {
    validate_bucket(s).is_ok()
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for BucketName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for BucketName {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for BucketName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for BucketName {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        BucketName::new(value)
    }
}

impl TryFrom<&str> for BucketName {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        BucketName::parse(value)
    }
}

impl FromStr for BucketName {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BucketName::parse(s)
    }
}

impl AsRef<str> for BucketName {
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
    fn valid_buckets() {
        assert!(BucketName::parse("my-bucket").is_ok());
        assert!(BucketName::parse("my.bucket123").is_ok());
        assert!(BucketName::parse("abc").is_ok());
        assert!(BucketName::parse("a-b.c").is_ok());
    }

    #[test]
    fn invalid_ip() {
        assert!(BucketName::parse("192.168.1.1").is_err());
    }

    #[test]
    fn invalid_start_hyphen() {
        assert!(BucketName::parse("-bucket").is_err());
    }

    #[test]
    fn invalid_char() {
        assert!(BucketName::parse("Bucket").is_err());
        assert!(BucketName::parse("my_bucket").is_err());
    }
}
