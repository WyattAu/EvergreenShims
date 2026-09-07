// vendored: allow pre-existing lints (upstream repos use different lint config)
#![allow(clippy::indexing_slicing, clippy::needless_return)]
//! BCP 47 locale tag newtype.

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// A validated BCP 47 locale tag, e.g. `en`, `en-US`, `es-419`.
///
/// Validation:
/// - max 35 characters
/// - regex `^[a-z]{2,3}(-[A-Za-z0-9]+)*$`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct LocaleTag(String);

impl LocaleTag {
    /// Parse and validate a locale tag.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidLocale`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_locale(s)
    }

    /// Create from an owned string.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidLocale`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        validate_locale(&s)
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

fn validate_locale(input: &str) -> Result<LocaleTag, ValidError> {
    if input.is_empty() {
        return Err(ValidError::InvalidLocale("locale is empty".to_string()));
    }
    if input.len() > 35 {
        return Err(ValidError::InvalidLocale(alloc::format!(
            "locale exceeds 35 characters, got {}",
            input.len()
        )));
    }
    if input.contains('\r') || input.contains('\n') {
        return Err(ValidError::InvalidLocale(
            "locale contains CR or LF".to_string(),
        ));
    }

    #[cfg(feature = "regex")]
    {
        use std::sync::OnceLock;
        static RE: OnceLock<regex::Regex> = OnceLock::new();
        let re = match RE.get() {
            Some(r) => r,
            None => {
                let init = match regex::Regex::new(r"^[a-z]{2,3}(-[A-Za-z0-9]+)*$") {
                    Ok(r) => r,
                    Err(_) => {
                        return Err(ValidError::InvalidLocale(
                            "internal regex error".to_string(),
                        ))
                    }
                };
                let _ = RE.set(init);
                match RE.get() {
                    Some(r) => r,
                    None => {
                        return Err(ValidError::InvalidLocale(
                            "internal regex error".to_string(),
                        ))
                    }
                }
            }
        };
        if !re.is_match(input) {
            return Err(ValidError::InvalidLocale(alloc::format!(
                "locale '{}' does not match BCP47 pattern",
                input
            )));
        }
        Ok(LocaleTag(input.to_string()))
    }

    #[cfg(not(feature = "regex"))]
    {
        let mut parts = input.split('-');
        let first = match parts.next() {
            Some(p) => p,
            None => return Err(ValidError::InvalidLocale("locale is empty".to_string())),
        };
        if first.len() < 2 || first.len() > 3 {
            return Err(ValidError::InvalidLocale(alloc::format!(
                "locale primary tag must be 2-3 chars, got '{}'",
                first
            )));
        }
        if !first.chars().all(|c| c.is_ascii_lowercase()) {
            return Err(ValidError::InvalidLocale(alloc::format!(
                "locale primary tag must be lowercase a-z, got '{}'",
                first
            )));
        }
        for subtag in parts {
            if subtag.is_empty() {
                return Err(ValidError::InvalidLocale(
                    "locale contains empty subtag".to_string(),
                ));
            }
            if subtag.len() > 8 {
                return Err(ValidError::InvalidLocale(alloc::format!(
                    "locale subtag '{}' exceeds 8 characters",
                    subtag
                )));
            }
            if !subtag.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(ValidError::InvalidLocale(alloc::format!(
                    "locale subtag '{}' must be alphanumeric",
                    subtag
                )));
            }
        }
        return Ok(LocaleTag(input.to_string()));
    }
}

/// Returns `true` if `s` is a valid locale tag.
#[must_use]
pub fn is_valid_locale(s: &str) -> bool {
    validate_locale(s).is_ok()
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for LocaleTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for LocaleTag {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for LocaleTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for LocaleTag {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        LocaleTag::new(value)
    }
}

impl TryFrom<&str> for LocaleTag {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        LocaleTag::parse(value)
    }
}

impl FromStr for LocaleTag {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        LocaleTag::parse(s)
    }
}

impl AsRef<str> for LocaleTag {
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
    fn valid_locales() {
        assert!(LocaleTag::parse("en").is_ok());
        assert!(LocaleTag::parse("en-US").is_ok());
        assert!(LocaleTag::parse("es-419").is_ok());
        assert!(LocaleTag::parse("zh-Hans-CN").is_ok());
    }

    #[test]
    fn invalid_uppercase_primary() {
        assert!(LocaleTag::parse("EN").is_err());
    }

    #[test]
    fn invalid_empty_subtag() {
        assert!(LocaleTag::parse("en-").is_err());
        assert!(LocaleTag::parse("en--US").is_err());
    }

    #[test]
    fn invalid_too_long() {
        let long = "en-".to_string() + &"a".repeat(33);
        assert!(LocaleTag::parse(&long).is_err());
    }
}
