// vendored: allow pre-existing lints (upstream repos use different lint config)
#![allow(clippy::indexing_slicing, clippy::needless_return)]
//! E.164 phone number newtype.
//!
//! Validates the format `^\+[1-9]\d{1,14}$` when the `regex` feature is enabled.
//! Without `regex`, a hand-rolled check is used.

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// A validated E.164 phone number, e.g. `+14155552671`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct PhoneE164(String);

impl PhoneE164 {
    /// Parse and validate an E.164 phone number.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidPhone`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_phone(s)
    }

    /// Create from an owned string.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidPhone`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        validate_phone(&s)
    }

    /// Return the phone number as a string slice.
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

fn validate_phone(input: &str) -> Result<PhoneE164, ValidError> {
    if input.is_empty() {
        return Err(ValidError::InvalidPhone("phone is empty".to_string()));
    }
    if input.contains('\r') || input.contains('\n') || input.contains(' ') {
        return Err(ValidError::InvalidPhone(
            "phone must not contain whitespace or control".to_string(),
        ));
    }

    // Prefer regex when available for exact spec compliance.
    #[cfg(feature = "regex")]
    {
        // Compiling regex on each call is not ideal for perf but avoids global state without once_cell.
        // Use a cached regex via std::sync::OnceLock when std is available.
        #[cfg(feature = "std")]
        {
            use std::sync::OnceLock;
            static RE: OnceLock<regex::Regex> = OnceLock::new();
            let re = match RE.get() {
                Some(r) => r,
                None => {
                    let init = match regex::Regex::new(r"^\+[1-9]\d{1,14}$") {
                        Ok(r) => r,
                        Err(_) => {
                            return Err(ValidError::InvalidPhone(
                                "internal regex error".to_string(),
                            ))
                        }
                    };
                    let _ = RE.set(init);
                    match RE.get() {
                        Some(r) => r,
                        None => {
                            return Err(ValidError::InvalidPhone(
                                "internal regex error".to_string(),
                            ))
                        }
                    }
                }
            };
            if !re.is_match(input) {
                return Err(ValidError::InvalidPhone(
                    "phone must match E.164 format +[1-9] followed by 1-14 digits".to_string(),
                ));
            }
        }
        #[cfg(not(feature = "std"))]
        {
            // Without std, compile each time (no OnceLock).
            let re = match regex::Regex::new(r"^\+[1-9]\d{1,14}$") {
                Ok(r) => r,
                Err(_) => return Err(ValidError::InvalidPhone("internal regex error".to_string())),
            };
            if !re.is_match(input) {
                return Err(ValidError::InvalidPhone(
                    "phone must match E.164 format +[1-9] followed by 1-14 digits".to_string(),
                ));
            }
        }
        Ok(PhoneE164(input.to_string()))
    }

    #[cfg(not(feature = "regex"))]
    {
        // Hand-rolled: ^\+[1-9]\d{1,14}$
        let bytes = input.as_bytes();
        if bytes[0] != b'+' {
            return Err(ValidError::InvalidPhone(
                "phone must start with '+'".to_string(),
            ));
        }
        let digits = &bytes[1..];
        if digits.len() < 2 || digits.len() > 15 {
            // 1-14 after first digit? Actually spec says \d{1,14} after [1-9], total digits 2-15 after '+', total length 3-16.
            // But spec regex ^\+[1-9]\d{1,14}$ means total digits after + is 2..15.
            return Err(ValidError::InvalidPhone(
                "phone must have 2-15 digits after '+'".to_string(),
            ));
        }
        if digits[0] == b'0' {
            return Err(ValidError::InvalidPhone(
                "phone must not start with 0 after '+'".to_string(),
            ));
        }
        for &b in digits {
            if !b.is_ascii_digit() {
                return Err(ValidError::InvalidPhone(
                    "phone must contain only digits after '+'".to_string(),
                ));
            }
        }
        return Ok(PhoneE164(input.to_string()));
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PhoneE164 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for PhoneE164 {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for PhoneE164 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for PhoneE164 {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        PhoneE164::new(value)
    }
}

impl TryFrom<&str> for PhoneE164 {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        PhoneE164::parse(value)
    }
}

impl FromStr for PhoneE164 {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PhoneE164::parse(s)
    }
}

impl AsRef<str> for PhoneE164 {
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
    fn valid_e164() {
        assert!(PhoneE164::parse("+14155552671").is_ok());
        assert!(PhoneE164::parse("+442071838750").is_ok());
        assert!(PhoneE164::parse("+12").is_ok());
    }

    #[test]
    fn invalid_no_plus() {
        assert!(PhoneE164::parse("14155552671").is_err());
    }

    #[test]
    fn invalid_zero_start() {
        assert!(PhoneE164::parse("+0123").is_err());
    }

    #[test]
    fn invalid_too_long() {
        assert!(PhoneE164::parse("+1234567890123456").is_err());
    }
}
