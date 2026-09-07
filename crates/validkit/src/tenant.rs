//! Tenant ID slug newtype.

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// A validated tenant ID slug.
///
/// Validation (matches `suture` `BranchName` logic):
/// - 3–63 characters
/// - lowercase alphanumeric + hyphen
/// - must not start or end with hyphen
/// - must not contain `..`
/// - must not end with `.lock` suffix
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct TenantIdSlug(String);

impl TenantIdSlug {
    /// Parse and validate a tenant ID.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidTenantId`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_tenant(s)
    }

    /// Create from an owned string.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidTenantId`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        validate_tenant(&s)
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

fn validate_tenant(input: &str) -> Result<TenantIdSlug, ValidError> {
    if input.len() < 3 || input.len() > 63 {
        return Err(ValidError::InvalidTenantId(alloc::format!(
            "tenant id must be 3-63 characters, got {}",
            input.len()
        )));
    }
    if input.contains('\r') || input.contains('\n') {
        return Err(ValidError::InvalidTenantId(
            "tenant id contains CR or LF".to_string(),
        ));
    }
    if input.starts_with('-') || input.ends_with('-') {
        return Err(ValidError::InvalidTenantId(
            "tenant id must not start or end with '-'".to_string(),
        ));
    }
    if input.contains("..") {
        return Err(ValidError::InvalidTenantId(
            "tenant id must not contain '..'".to_string(),
        ));
    }
    if input.ends_with(".lock") {
        return Err(ValidError::InvalidTenantId(
            "tenant id must not end with '.lock'".to_string(),
        ));
    }
    // Must be lowercase alphanumeric + hyphen only.
    for ch in input.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(ValidError::InvalidTenantId(alloc::format!(
                "invalid character '{}' in tenant id",
                ch
            )));
        }
    }
    // Also disallow consecutive hyphens? Not required by spec, but keep allowed.
    // Ensure not purely hyphens? Already covered by start/end hyphen + allowed chars.
    Ok(TenantIdSlug(input.to_string()))
}

/// Returns `true` if `s` is a valid tenant id slug.
#[must_use]
pub fn is_valid_tenant_id(s: &str) -> bool {
    validate_tenant(s).is_ok()
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for TenantIdSlug {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for TenantIdSlug {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for TenantIdSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for TenantIdSlug {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        TenantIdSlug::new(value)
    }
}

impl TryFrom<&str> for TenantIdSlug {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        TenantIdSlug::parse(value)
    }
}

impl FromStr for TenantIdSlug {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        TenantIdSlug::parse(s)
    }
}

impl AsRef<str> for TenantIdSlug {
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
    fn valid_tenant() {
        assert!(TenantIdSlug::parse("abc").is_ok());
        assert!(TenantIdSlug::parse("my-tenant-123").is_ok());
        assert!(TenantIdSlug::parse("a-b-c").is_ok());
    }

    #[test]
    fn invalid_short() {
        assert!(TenantIdSlug::parse("ab").is_err());
    }

    #[test]
    fn invalid_uppercase() {
        assert!(TenantIdSlug::parse("Abc").is_err());
    }

    #[test]
    fn invalid_lock_suffix() {
        assert!(TenantIdSlug::parse("foo.lock").is_err());
    }

    #[test]
    fn invalid_double_dot() {
        assert!(TenantIdSlug::parse("a..b").is_err());
    }
}
