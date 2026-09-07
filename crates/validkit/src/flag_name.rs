// vendored: allow pre-existing lints (upstream repos use different lint config)
#![allow(clippy::indexing_slicing, clippy::needless_return)]
//! Flag name newtype.

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// A validated flag name.
///
/// Validation: `^[a-z][a-z0-9_]*$`
/// Snake-case, starting with a lowercase letter, containing only lowercase
/// alphanumeric and underscore (e.g. `flag_ai_analyzers` lowercased form).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct FlagName(String);

impl FlagName {
    /// Parse and validate a flag name.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidFlagName`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_flag(s)
    }

    /// Create from an owned string.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidFlagName`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        validate_flag(&s)
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

fn validate_flag(input: &str) -> Result<FlagName, ValidError> {
    if input.is_empty() {
        return Err(ValidError::InvalidFlagName);
    }
    if input.contains('\r') || input.contains('\n') {
        return Err(ValidError::InvalidFlagName);
    }

    #[cfg(feature = "regex")]
    {
        use std::sync::OnceLock;
        static RE: OnceLock<regex::Regex> = OnceLock::new();
        let re = match RE.get() {
            Some(r) => r,
            None => {
                let init = match regex::Regex::new(r"^[a-z][a-z0-9_]*$") {
                    Ok(r) => r,
                    Err(_) => return Err(ValidError::InvalidFlagName),
                };
                let _ = RE.set(init);
                match RE.get() {
                    Some(r) => r,
                    None => return Err(ValidError::InvalidFlagName),
                }
            }
        };
        if !re.is_match(input) {
            return Err(ValidError::InvalidFlagName);
        }
        Ok(FlagName(input.to_string()))
    }

    #[cfg(not(feature = "regex"))]
    {
        let mut chars = input.chars();
        let first = match chars.next() {
            Some(c) => c,
            None => return Err(ValidError::InvalidFlagName),
        };
        if !first.is_ascii_lowercase() {
            return Err(ValidError::InvalidFlagName);
        }
        for ch in chars {
            if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '_' {
                return Err(ValidError::InvalidFlagName);
            }
        }
        return Ok(FlagName(input.to_string()));
    }
}

/// Returns `true` if `s` is a valid flag name.
#[must_use]
pub fn is_valid_flag_name(s: &str) -> bool {
    validate_flag(s).is_ok()
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for FlagName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for FlagName {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for FlagName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for FlagName {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        FlagName::new(value)
    }
}

impl TryFrom<&str> for FlagName {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        FlagName::parse(value)
    }
}

impl FromStr for FlagName {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        FlagName::parse(s)
    }
}

impl AsRef<str> for FlagName {
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
    fn valid_flags() {
        assert!(FlagName::parse("a").is_ok());
        assert!(FlagName::parse("flag_a").is_ok());
        assert!(FlagName::parse("ai_analyzers").is_ok());
        assert!(FlagName::parse("flag2").is_ok());
    }

    #[test]
    fn invalid_uppercase() {
        assert!(FlagName::parse("Flag").is_err());
    }

    #[test]
    fn invalid_start_digit() {
        assert!(FlagName::parse("1flag").is_err());
    }

    #[test]
    fn invalid_hyphen() {
        assert!(FlagName::parse("flag-name").is_err());
    }
}
