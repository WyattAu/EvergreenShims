#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! # validkit
//!
//! Typed newtypes for validated domain primitives — replaces hand-rolled
//! `is_valid_*` checks with parse-once, use-everywhere strong types.
//!
//! | Type | Validation |
//! |------|------------|
//! | [`EmailAddr`] | RFC 5321 + IDNA, injection-safe |
//! | [`HttpsUrl`] | `https` only, no credentials, host required |
//! | [`PhoneE164`] | E.164 `^\+[1-9]\d{1,14}$` |
//! | [`CronExpr`] | 5-field cron (`*` or `[\d,/\-]+`) |
//! | [`TenantIdSlug`] | `3–63` chars, `[a-z0-9-]`, no `..`, no `.lock` |
//! | [`LocaleTag`] | BCP 47 `^[a-z]{2,3}(-[A-Za-z0-9]+)*$`, max 35 |
//! | [`FlagName`] | `^[a-z][a-z0-9_]*$` |
//! | [`BucketName`] | S3 bucket rules, not IP |
//! | [`ObjectKey`] | S3 object key, no `..` traversal, ≤1024, no leading `/` |
//!
//! ## Example
//!
//! ```rust
//! use validkit::{EmailAddr, HttpsUrl, TenantIdSlug};
//!
//! let email = EmailAddr::parse("alice@example.com").expect("valid");
//! assert_eq!(email.as_str(), "alice@example.com");
//!
//! let url = HttpsUrl::parse("https://example.com").expect("valid");
//! assert!(url.as_str().starts_with("https://"));
//!
//! let tenant = TenantIdSlug::parse("my-tenant").expect("valid");
//! assert_eq!(tenant.as_str(), "my-tenant");
//! ```
//!
//! All types implement `TryFrom<String>`, `FromStr`, `Display`, `Deref<Target=str>`,
//! and optional `serde` support (transparent serialization; deserialization
//! runs full validation, so invalid values cannot enter through JSON).
//!

extern crate alloc;

pub mod bucket;
pub mod cron;
pub mod email;
pub mod error;
pub mod flag_name;
pub mod locale;
pub mod object_key;
pub mod phone;
pub mod tenant;
pub mod url;

// Re-exports
pub use bucket::is_valid_bucket_name;
pub use bucket::BucketName;
pub use cron::is_valid_cron;
pub use cron::CronExpr;
pub use email::is_valid_email;
pub use email::EmailAddr;
pub use email::Validate;
pub use error::ValidError;
pub use flag_name::is_valid_flag_name;
pub use flag_name::FlagName;
pub use locale::is_valid_locale;
pub use locale::LocaleTag;
pub use object_key::is_valid_object_key;
pub use object_key::ObjectKey;
pub use phone::PhoneE164;
pub use tenant::is_valid_tenant_id;
pub use tenant::TenantIdSlug;
pub use url::HttpsUrl;

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
mod smoke {
    use super::*;

    #[test]
    fn reexports_work() {
        let _ = EmailAddr::parse("a@b.co").expect("valid");
        let _ = TenantIdSlug::parse("abc").expect("valid");
        let _ = LocaleTag::parse("en-US").expect("valid");
        let _ = FlagName::parse("my_flag").expect("valid");
        let _ = BucketName::parse("my-bucket").expect("valid");
        let _ = ObjectKey::parse("a/b/c").expect("valid");
        let _ = CronExpr::parse("* * * * *").expect("valid");
        let _ = PhoneE164::parse("+1234567890").expect("valid");
    }
}
