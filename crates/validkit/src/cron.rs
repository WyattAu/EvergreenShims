//! Cron expression newtype (5-field).

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// A validated 5-field cron expression.
///
/// Validation is deliberately simple: it checks that the expression consists
/// of exactly five whitespace-separated fields, each matching
/// `*` or `[\d,/\-]+` (digits, commas, slashes, hyphens) and additional
/// syntactic sanity (no empty fields). This is intentionally not a full cron
/// parser; it catches the common class of hand-rolled `is_valid_cron` errors
/// without pulling in a heavy parser.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct CronExpr(String);

impl CronExpr {
    /// Parse and validate a cron expression.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidCron`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_cron(s)
    }

    /// Create from an owned string.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidCron`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        validate_cron(&s)
    }

    /// Return the cron expression as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume and return the inner string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

fn validate_cron(input: &str) -> Result<CronExpr, ValidError> {
    if input.is_empty() {
        return Err(ValidError::InvalidCron("cron is empty".to_string()));
    }
    if input.contains('\r') || input.contains('\n') {
        return Err(ValidError::InvalidCron(
            "cron contains CR or LF".to_string(),
        ));
    }
    // Trim and split on whitespace (any number of spaces/tabs).
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ValidError::InvalidCron("cron is empty".to_string()));
    }
    let fields: alloc::vec::Vec<&str> = trimmed.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(ValidError::InvalidCron(alloc::format!(
            "cron must have exactly 5 fields, got {}",
            fields.len()
        )));
    }

    // Validate each field.
    // When regex feature is enabled, use the specified regex pattern.
    #[cfg(feature = "regex")]
    {
        use std::sync::OnceLock;
        // Regex per field: allow digits, *, comma, slash, hyphen combinations.
        // Spec snippet ^(\*|[\d,/\-]+) is simplified; we extend to support common
        // cron like "*/5" by allowing "*\/\d" combos: ^[\d\*,\/\-]+$
        static FIELD_RE: OnceLock<regex::Regex> = OnceLock::new();
        let re = match FIELD_RE.get() {
            Some(r) => r,
            None => {
                let init = match regex::Regex::new(r"^[\d\*,\/\-]+$") {
                    Ok(r) => r,
                    Err(_) => {
                        return Err(ValidError::InvalidCron("internal regex error".to_string()))
                    }
                };
                let _ = FIELD_RE.set(init);
                match FIELD_RE.get() {
                    Some(r) => r,
                    None => {
                        return Err(ValidError::InvalidCron("internal regex error".to_string()))
                    }
                }
            }
        };
        for field in &fields {
            if !re.is_match(field) {
                return Err(ValidError::InvalidCron(alloc::format!(
                    "invalid cron field '{}'",
                    field
                )));
            }
            // Additional checks: require digit or '*' if punctuation present, and no leading/trailing punctuation
            if (field.contains(',') || field.contains('/') || field.contains('-'))
                && !field.chars().any(|c| c.is_ascii_digit() || c == '*')
            {
                return Err(ValidError::InvalidCron(alloc::format!(
                    "invalid cron field '{}'",
                    field
                )));
            }
            if field.starts_with('/') || field.starts_with(',') || field.starts_with('-') {
                return Err(ValidError::InvalidCron(alloc::format!(
                    "invalid cron field '{}'",
                    field
                )));
            }
            if field.ends_with('/') || field.ends_with(',') || field.ends_with('-') {
                return Err(ValidError::InvalidCron(alloc::format!(
                    "invalid cron field '{}'",
                    field
                )));
            }
            if field.contains("//") || field.contains(",,") || field.contains("--") {
                return Err(ValidError::InvalidCron(alloc::format!(
                    "invalid cron field '{}'",
                    field
                )));
            }
        }
    }

    #[cfg(not(feature = "regex"))]
    {
        for field in &fields {
            if field.is_empty() {
                return Err(ValidError::InvalidCron("cron field is empty".to_string()));
            }
            if *field == "*" {
                continue;
            }
            // Must consist only of digits, ',', '/', '-', '*'
            let mut has_digit = false;
            for ch in field.chars() {
                if ch.is_ascii_digit() {
                    has_digit = true;
                    continue;
                }
                if matches!(ch, ',' | '/' | '-' | '*') {
                    continue;
                }
                return Err(ValidError::InvalidCron(alloc::format!(
                    "invalid character '{}' in cron field '{}'",
                    ch,
                    field
                )));
            }
            if !has_digit && !field.contains('*') {
                return Err(ValidError::InvalidCron(alloc::format!(
                    "invalid cron field '{}'",
                    field
                )));
            }
            // Prevent leading/trailing and consecutive punctuation
            if field.starts_with(',')
                || field.starts_with('/')
                || field.starts_with('-')
                || field.ends_with(',')
                || field.ends_with('/')
                || field.ends_with('-')
                || field.contains(",,")
                || field.contains("//")
                || field.contains("--")
            {
                return Err(ValidError::InvalidCron(alloc::format!(
                    "invalid cron field '{}'",
                    field
                )));
            }
        }
    }

    Ok(CronExpr(trimmed.to_string()))
}

/// Returns `true` if `s` is a valid 5-field cron expression.
#[must_use]
pub fn is_valid_cron(s: &str) -> bool {
    validate_cron(s).is_ok()
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for CronExpr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for CronExpr {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for CronExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for CronExpr {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        CronExpr::new(value)
    }
}

impl TryFrom<&str> for CronExpr {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        CronExpr::parse(value)
    }
}

impl FromStr for CronExpr {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CronExpr::parse(s)
    }
}

impl AsRef<str> for CronExpr {
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
    fn valid_cron_simple() {
        assert!(CronExpr::parse("* * * * *").is_ok());
        assert!(CronExpr::parse("0 0 * * 0").is_ok());
        assert!(CronExpr::parse("*/5 * * * *").is_ok());
        assert!(CronExpr::parse("0 0 1,15 * 1-5").is_ok());
    }

    #[test]
    fn invalid_cron_wrong_fields() {
        assert!(CronExpr::parse("* * * *").is_err());
        assert!(CronExpr::parse("* * * * * *").is_err());
    }

    #[test]
    fn invalid_cron_bad_char() {
        assert!(CronExpr::parse("a * * * *").is_err());
    }
}
