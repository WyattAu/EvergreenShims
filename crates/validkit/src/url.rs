//! HTTPS URL newtype.

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::error::ValidError;

/// A validated HTTPS URL.
///
/// Guarantees:
/// - scheme is `https`
/// - host is present
/// - no credentials (username/password) are present
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct HttpsUrl(
    #[cfg(feature = "url")] pub(crate) url::Url,
    #[cfg(not(feature = "url"))] String,
);

impl HttpsUrl {
    /// Parse and validate an HTTPS URL from a string.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidUrl`] if validation fails.
    pub fn new(s: String) -> Result<Self, ValidError> {
        Self::parse(&s)
    }

    /// Parse from `&str`.
    ///
    /// # Errors
    ///
    /// Returns [`ValidError::InvalidUrl`] if validation fails.
    pub fn parse(s: &str) -> Result<Self, ValidError> {
        validate_https_url(s)
    }

    /// Return the URL as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        #[cfg(feature = "url")]
        {
            self.0.as_str()
        }
        #[cfg(not(feature = "url"))]
        {
            &self.0
        }
    }

    /// Consume and return the inner string.
    #[must_use]
    pub fn into_inner(self) -> String {
        #[cfg(feature = "url")]
        {
            self.0.to_string()
        }
        #[cfg(not(feature = "url"))]
        {
            self.0
        }
    }

    /// Return the inner [`url::Url`] when the `url` feature is enabled.
    #[cfg(feature = "url")]
    #[must_use]
    pub fn as_url(&self) -> &url::Url {
        &self.0
    }
}

fn validate_https_url(input: &str) -> Result<HttpsUrl, ValidError> {
    if input.is_empty() {
        return Err(ValidError::InvalidUrl("url is empty".to_string()));
    }
    if input.contains('\r') || input.contains('\n') {
        return Err(ValidError::InvalidUrl("url contains CR or LF".to_string()));
    }

    #[cfg(feature = "url")]
    {
        let parsed = url::Url::parse(input).map_err(|e| ValidError::InvalidUrl(e.to_string()))?;

        if parsed.scheme() != "https" {
            return Err(ValidError::InvalidUrl(
                "url scheme must be https".to_string(),
            ));
        }
        if parsed.host_str().is_none() {
            return Err(ValidError::InvalidUrl("url host is required".to_string()));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(ValidError::InvalidUrl(
                "url must not contain credentials".to_string(),
            ));
        }
        // Disallow userinfo via @ in host? url crate already parsed it.
        Ok(HttpsUrl(parsed))
    }

    #[cfg(not(feature = "url"))]
    {
        // Fallback without `url` crate: minimal checks.
        if !input.starts_with("https://") {
            return Err(ValidError::InvalidUrl(
                "url scheme must be https".to_string(),
            ));
        }
        let after_scheme = &input["https://".len()..];
        if after_scheme.is_empty() {
            return Err(ValidError::InvalidUrl("url host is required".to_string()));
        }
        // No credentials: disallow '@' before first '/'.
        let host_part = match after_scheme.split('/').next() {
            Some(p) => p,
            None => "",
        };
        if host_part.contains('@') {
            return Err(ValidError::InvalidUrl(
                "url must not contain credentials".to_string(),
            ));
        }
        if host_part.is_empty() {
            return Err(ValidError::InvalidUrl("url host is required".to_string()));
        }
        if input.contains(' ') {
            return Err(ValidError::InvalidUrl("url contains space".to_string()));
        }
        Ok(HttpsUrl(input.to_string()))
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for HttpsUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl Deref for HttpsUrl {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for HttpsUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for HttpsUrl {
    type Error = ValidError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        HttpsUrl::new(value)
    }
}

impl TryFrom<&str> for HttpsUrl {
    type Error = ValidError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        HttpsUrl::parse(value)
    }
}

impl FromStr for HttpsUrl {
    type Err = ValidError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HttpsUrl::parse(s)
    }
}

impl AsRef<str> for HttpsUrl {
    fn as_ref(&self) -> &str {
        self.as_str()
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
#[cfg(all(test, feature = "url"))]
mod tests {
    use super::*;

    #[test]
    fn valid_https() {
        let u = HttpsUrl::parse("https://example.com/path?q=1").expect("valid https");
        assert_eq!(u.as_str(), "https://example.com/path?q=1");
    }

    #[test]
    fn invalid_http() {
        assert!(HttpsUrl::parse("http://example.com").is_err());
    }

    #[test]
    fn invalid_no_host() {
        assert!(HttpsUrl::parse("https://").is_err());
    }

    #[test]
    fn invalid_credentials() {
        assert!(HttpsUrl::parse("https://user:pass@example.com").is_err());
    }
}
