//! Email address newtype with strict validation.
//!
//! Validation is intentionally conservative: it prevents header injection
//! (`\r` / `\n`), enforces a single `@`, validates local-part and domain
//! length limits, and IDNA-encodes the domain.

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// Trait for types that can validate themselves.
pub trait Validate {
    /// Validate `self`, returning an error if invalid.
    fn validate(&self) -> Result<(), ValidError>;
}

/// A validated email address.
///
/// Display normalises the domain part to lowercase and, when the `idna`
/// feature is enabled, to ASCII (punycode) form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct EmailAddr(String);

impl EmailAddr {
    /// Parse and validate an email address from a string slice.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidEmail`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_email(s)
    }

    /// Create a new `EmailAddr` from an owned string, validating it.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidEmail`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        validate_email(&s)
    }

    /// Return the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume `self` and return the inner `String`.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// Returns `true` if `s` is a valid email address.
#[must_use]
pub fn is_valid_email(s: &str) -> bool {
    validate_email(s).is_ok()
}

fn validate_email(input: &str) -> Result<EmailAddr, ValidError> {
    if input.is_empty() {
        return Err(ValidError::InvalidEmail("email is empty".to_string()));
    }
    if input.len() > 254 {
        return Err(ValidError::InvalidEmail(
            "email exceeds 254 characters".to_string(),
        ));
    }
    // Prevent header injection.
    if input.contains('\r') || input.contains('\n') {
        return Err(ValidError::InvalidEmail(
            "email contains CR or LF".to_string(),
        ));
    }
    // Must contain exactly one '@'.
    let at_count = input.chars().filter(|&c| c == '@').count();
    if at_count != 1 {
        return Err(ValidError::InvalidEmail(
            "email must contain exactly one @".to_string(),
        ));
    }
    #[allow(clippy::manual_unwrap_or, clippy::manual_unwrap_or_default)]
    let at_pos = match input.find('@') {
        Some(p) => p,
        None => 0,
    };
    let local = &input[..at_pos];
    let domain = &input[at_pos + 1..];

    // Local part checks (RFC 5321: max 64 octets).
    if local.is_empty() {
        return Err(ValidError::InvalidEmail("local part is empty".to_string()));
    }
    if local.len() > 64 {
        return Err(ValidError::InvalidEmail(
            "local part exceeds 64 characters".to_string(),
        ));
    }
    if local.starts_with('.') || local.ends_with('.') {
        return Err(ValidError::InvalidEmail(
            "local part must not start or end with '.'".to_string(),
        ));
    }
    if local.contains("..") {
        return Err(ValidError::InvalidEmail(
            "local part must not contain consecutive dots".to_string(),
        ));
    }
    // Local part allowed characters: alphanumeric plus !#$%&'*+/=?^_`{|}~-. (dot already handled)
    // For conservatism we allow a-zA-Z0-9 and the special set, disallow spaces/control.
    for ch in local.chars() {
        if ch.is_control() || ch == ' ' {
            return Err(ValidError::InvalidEmail(
                "local part contains invalid character".to_string(),
            ));
        }
        // Disallow chars that would break quoting without full parser: <>()[]\\,;:
        // We do a simple check: only allow ASCII printable minus problematic ones, or allow full?
        // Keep permissive but block clearly invalid.
        if matches!(
            ch,
            '<' | '>' | '(' | ')' | '[' | ']' | '\\' | ',' | ';' | ':' | '"'
        ) {
            return Err(ValidError::InvalidEmail(alloc::format!(
                "local part contains invalid character '{}'",
                ch
            )));
        }
    }

    // Domain checks.
    if domain.is_empty() {
        return Err(ValidError::InvalidEmail("domain is empty".to_string()));
    }
    if domain.len() > 253 {
        return Err(ValidError::InvalidEmail(
            "domain exceeds 253 characters".to_string(),
        ));
    }
    if domain.contains("..") {
        return Err(ValidError::InvalidEmail(
            "domain must not contain consecutive dots".to_string(),
        ));
    }
    if domain.starts_with('.')
        || domain.ends_with('.')
        || domain.starts_with('-')
        || domain.ends_with('-')
    {
        return Err(ValidError::InvalidEmail(
            "domain must not start or end with '.' or '-'".to_string(),
        ));
    }
    if domain.contains(' ') || domain.contains('\t') {
        return Err(ValidError::InvalidEmail(
            "domain contains whitespace".to_string(),
        ));
    }
    // Domain must contain at least one dot unless it's a single label? Require dot for safety.
    // But allow single-label for tests? We allow single label but must be valid label.
    // Validate labels.
    let labels: Vec<&str> = domain.split('.').collect();
    for label in &labels {
        if label.is_empty() {
            return Err(ValidError::InvalidEmail(
                "domain contains empty label".to_string(),
            ));
        }
        if label.len() > 63 {
            return Err(ValidError::InvalidEmail(
                "domain label exceeds 63 characters".to_string(),
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(ValidError::InvalidEmail(
                "domain label must not start or end with '-'".to_string(),
            ));
        }
        // Labels must be alphanumeric + hyphen only after IDNA? Check basic.
        // Allow IDNA unicode labels; they will be encoded later.
    }

    // IDNA validation / normalisation of domain.
    let normalised_domain: String;
    #[cfg(feature = "idna")]
    {
        // Use idna crate to validate and convert to ASCII.
        // idna::domain_to_ascii is the recommended entrypoint.
        match idna::domain_to_ascii(domain) {
            Ok(ascii) => {
                // domain_to_ascii lowercases and validates.
                // Additional check: ascii must not be empty.
                if ascii.is_empty() {
                    return Err(ValidError::InvalidEmail(
                        "domain IDNA conversion produced empty string".to_string(),
                    ));
                }
                normalised_domain = ascii;
            }
            Err(_) => {
                return Err(ValidError::InvalidEmail(
                    "domain failed IDNA validation".to_string(),
                ));
            }
        }
    }
    #[cfg(not(feature = "idna"))]
    {
        // Without idna, lowercase ASCII domain and check ascii-only.
        // Allow unicode domains to pass through lowercased but require basic check.
        normalised_domain = domain.to_ascii_lowercase();
        // If domain contains non-ascii without idna, still allow but lowercase.
        // We do a simple ascii check for hyphen/label validity after lowercasing.
        // Non-ascii domains without idna feature are considered invalid to avoid bypass.
        if !domain.is_ascii() {
            // Without IDNA we cannot reliably validate; treat as invalid.
            return Err(ValidError::InvalidEmail(
                "non-ascii domain requires idna feature".to_string(),
            ));
        }
    }

    // Reconstruct normalised email: local part as-is, domain lowercased/ascii.
    // Local part is case-sensitive per RFC but we keep as provided.
    let normalised = alloc::format!("{}@{}", local, normalised_domain);

    // Final length check after normalisation.
    if normalised.len() > 254 {
        return Err(ValidError::InvalidEmail(
            "normalised email exceeds 254 characters".to_string(),
        ));
    }

    Ok(EmailAddr(normalised))
}

impl Validate for EmailAddr {
    fn validate(&self) -> Result<(), ValidError> {
        // Re-validate the inner string (defensive).
        validate_email(&self.0).map(|_| ())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for EmailAddr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for EmailAddr {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for EmailAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for EmailAddr {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        EmailAddr::new(value)
    }
}

impl TryFrom<&str> for EmailAddr {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        EmailAddr::parse(value)
    }
}

impl FromStr for EmailAddr {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EmailAddr::parse(s)
    }
}

impl AsRef<str> for EmailAddr {
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
    fn valid_email_lowercases_domain() {
        let e = EmailAddr::parse("User@EXAMPLE.COM").expect("valid email");
        assert_eq!(e.as_str(), "User@example.com");
    }

    #[test]
    fn valid_simple() {
        assert!(is_valid_email("a@b.co"));
        assert!(is_valid_email("foo.bar+tag@example.org"));
    }

    #[test]
    fn invalid_no_at() {
        assert!(!is_valid_email("no-at.example.com"));
    }

    #[test]
    fn invalid_injection() {
        assert!(!is_valid_email("a@b.com\r\nBcc: x@y.com"));
    }

    #[test]
    fn invalid_double_dot() {
        assert!(!is_valid_email("a..b@example.com"));
    }
}
