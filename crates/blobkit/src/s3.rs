//! S3 backend.
//!
//! With the `s3` feature enabled this is a real implementation on top of
//! `aws-sdk-s3` / `aws-config`:
//!
//! - `put` → `PutObject`
//! - `get` → `GetObject`
//! - `delete` → `DeleteObject`
//! - `exists` → `HeadObject`
//! - `presigned_url` → `GetObject` pre-signing
//!
//! Without the feature the module provides only configuration and type stubs
//! so that default builds stay light: every [`BlobStore`] method returns
//! [`BlobError::Unsupported`]. The stub intentionally mirrors the real API
//! (including `async` `S3Store::new` returning `Result`), so call sites
//! compile unchanged once the `s3` feature is enabled.
//!
//! # Example
//!
//! ```rust,no_run
//! use blobkit::s3::{CredentialsMode, S3Config, S3Store};
//! use blobkit::types::BucketName;
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), blobkit::error::BlobError> {
//! let cfg = S3Config::new(BucketName::new("my-bucket")?, "us-east-1")
//!     .with_path_style(true)
//!     .with_credentials(CredentialsMode::FromEnv)
//!     .with_timeout(Some(Duration::from_secs(30)));
//! let store = S3Store::new(cfg).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # S3-compatible stores
//!
//! `S3Config::with_endpoint` plus `S3Config::with_path_style(true)` targets
//! MinIO, LocalStack, SeaweedFS, and Cloudflare R2. Path style is *required*
//! by most local/edge stores.

extern crate alloc;

use alloc::boxed::Box;
#[cfg(feature = "s3")]
use alloc::format;
use alloc::string::String;
use async_trait::async_trait;
use bytes::Bytes;
use core::time::Duration;

use crate::error::{BlobError, Result};
use crate::store::BlobStore;
use crate::types::{BlobId, BucketName, ObjectKey};

#[cfg(feature = "s3")]
use url::Url;

/// How [`S3Store`] obtains credentials.
///
/// `FromEnv` and `FromProfile` rely on the standard `aws-config` provider
/// chain (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`,
/// IMDS, ...). `Static` supplies explicit keys, e.g. for MinIO/LocalStack
/// where no profile exists.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum CredentialsMode {
    /// Read credentials from the environment (`AWS_ACCESS_KEY_ID`,
    /// `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, container/IMDS).
    #[default]
    FromEnv,
    /// Read credentials from the shared config/credentials files
    /// (`~/.aws/credentials`, `~/.aws/config`) via `AWS_PROFILE`.
    FromProfile,
    /// Static access/secret key pair (optionally add a session token via the
    /// environment when using temporary credentials).
    ///
    /// `Debug` is manually implemented to redact the secret key
    /// (REQ-BLOBKIT-101: secrets never leak via diagnostics).
    Static {
        /// Access key ID.
        access_key: String,
        /// Secret access key (redacted in [`Debug`](std::fmt::Debug)).
        secret_key: String,
    },
}

impl std::fmt::Debug for CredentialsMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FromEnv => f.debug_struct("CredentialsMode::FromEnv").finish(),
            Self::FromProfile => f.debug_struct("CredentialsMode::FromProfile").finish(),
            Self::Static { access_key, .. } => f
                .debug_struct("CredentialsMode::Static")
                .field("access_key", access_key)
                .field("secret_key", &"[redacted]")
                .finish(),
        }
    }
}

/// Configuration for the S3 backend.
#[derive(Clone, PartialEq, Eq)]
pub struct S3Config {
    /// Target bucket.
    pub bucket: BucketName,
    /// AWS region, e.g. `"us-east-1"`.
    pub region: String,
    /// Optional custom endpoint (e.g. for MinIO, LocalStack, or R2).
    #[cfg(feature = "s3")]
    pub endpoint: Option<Url>,
    /// Optional custom endpoint as string when `s3` feature is disabled.
    #[cfg(not(feature = "s3"))]
    pub endpoint: Option<String>,
    /// Use path-style addressing (`https://endpoint/bucket/key`) instead of
    /// virtual-host style (`https://bucket.endpoint/key`).
    ///
    /// Required by most S3-compatible stores (MinIO, LocalStack, R2).
    pub path_style: bool,
    /// Credential resolution strategy. Defaults to [`CredentialsMode::FromEnv`].
    pub credentials_mode: CredentialsMode,
    /// Per-operation timeout applied to every S3 call. Defaults to 30s.
    pub timeout: Option<Duration>,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // REQ-BLOBKIT-101: credential material never reaches diagnostics.
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("path_style", &self.path_style)
            .field("credentials_mode", &self.credentials_mode)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl S3Config {
    /// Create a new config with the given bucket and region.
    #[must_use]
    pub fn new(bucket: BucketName, region: impl Into<String>) -> Self {
        Self {
            bucket,
            region: region.into(),
            endpoint: None,
            path_style: false,
            credentials_mode: CredentialsMode::default(),
            timeout: Some(Duration::from_secs(30)),
        }
    }

    /// Set a custom endpoint.
    ///
    /// Use this for S3-compatible stores (MinIO, SeaweedFS, Cloudflare R2).
    #[cfg(feature = "s3")]
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: Url) -> Self {
        self.endpoint = Some(endpoint);
        self
    }

    /// Set a custom endpoint (string form, when `s3` feature is disabled).
    #[cfg(not(feature = "s3"))]
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Enable or disable path-style addressing.
    #[must_use]
    pub fn with_path_style(mut self, path_style: bool) -> Self {
        self.path_style = path_style;
        self
    }

    /// Set the credential resolution strategy.
    #[must_use]
    pub fn with_credentials(mut self, mode: CredentialsMode) -> Self {
        self.credentials_mode = mode;
        self
    }

    /// Set the per-operation timeout (`None` disables the explicit timeout
    /// and falls back to the SDK default).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// The bucket name.
    #[must_use]
    pub fn bucket(&self) -> &BucketName {
        &self.bucket
    }

    /// The region.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }
}

/// Map a service error to a [`BlobError`] from its HTTP status and error code.
#[cfg(any(feature = "s3", test))]
fn map_service_error(status: Option<u16>, code: Option<&str>, display: String) -> BlobError {
    let not_found = status == Some(404)
        || matches!(
            code,
            Some("NoSuchKey") | Some("NotFound") | Some("NoSuchBucket")
        );
    if not_found {
        return BlobError::not_found(display);
    }
    let denied = status == Some(401)
        || status == Some(403)
        || matches!(
            code,
            Some("AccessDenied") | Some("InvalidAccessKeyId") | Some("SignatureDoesNotMatch")
        );
    if denied {
        return BlobError::permission_denied(display);
    }
    BlobError::Other(display)
}

/// S3-backed blob store.
#[derive(Debug, Clone)]
#[cfg(feature = "s3")]
pub struct S3Store {
    config: S3Config,
    client: aws_sdk_s3::Client,
}

/// S3-backed blob store (stub when the `s3` feature is disabled).
///
/// All operations return `Err(BlobError::Unsupported)`. The stub mirrors the
/// real API so enabling the `s3` feature requires no call-site changes.
#[derive(Debug, Clone)]
#[cfg(not(feature = "s3"))]
pub struct S3Store {
    config: S3Config,
}

#[cfg(feature = "s3")]
impl S3Store {
    /// Build a real S3 client from `config`.
    ///
    /// Credentials resolve per [`S3Config::credentials_mode`]; the region and
    /// endpoint override the ambient environment, and `force_path_style` /
    /// per-operation timeout are applied on top of the `aws-config` defaults.
    pub async fn new(config: S3Config) -> Result<Self> {
        let mut loader = aws_config::defaults(aws_sdk_s3::config::BehaviorVersion::latest());
        match &config.credentials_mode {
            CredentialsMode::Static {
                access_key,
                secret_key,
            } => {
                loader = loader.credentials_provider(aws_sdk_s3::config::Credentials::new(
                    access_key.clone(),
                    secret_key.clone(),
                    None,
                    None,
                    "blobkit",
                ));
            }
            // FromEnv / FromProfile: rely on the aws-config default chain
            // (env vars, shared profile files, IMDS, container credentials).
            CredentialsMode::FromEnv | CredentialsMode::FromProfile => {}
        }
        loader = loader.region(aws_sdk_s3::config::Region::new(config.region.clone()));
        let sdk_conf = loader.load().await;

        let mut s3_conf = aws_sdk_s3::Config::from(&sdk_conf).to_builder();
        s3_conf.set_force_path_style(Some(config.path_style));
        if let Some(endpoint) = &config.endpoint {
            s3_conf.set_endpoint_url(Some(endpoint.to_string()));
        }
        if let Some(timeout) = config.timeout {
            s3_conf.set_timeout_config(Some(
                aws_smithy_types::timeout::TimeoutConfig::builder()
                    .operation_timeout(timeout)
                    .build(),
            ));
        }

        Ok(Self {
            config,
            client: aws_sdk_s3::Client::from_conf(s3_conf.build()),
        })
    }

    /// Borrow the config.
    #[must_use]
    pub fn config(&self) -> &S3Config {
        &self.config
    }

    /// Create a config and store in one call.
    pub async fn from_bucket_region(bucket: BucketName, region: impl Into<String>) -> Result<Self> {
        Self::new(S3Config::new(bucket, region)).await
    }
}

#[cfg(not(feature = "s3"))]
impl S3Store {
    /// Create a stub store from `config` (API-compatible with the real one).
    pub async fn new(config: S3Config) -> Result<Self> {
        Ok(Self { config })
    }

    /// Borrow the config.
    #[must_use]
    pub fn config(&self) -> &S3Config {
        &self.config
    }

    /// Create a config and store in one call.
    pub async fn from_bucket_region(bucket: BucketName, region: impl Into<String>) -> Result<Self> {
        Self::new(S3Config::new(bucket, region)).await
    }
}

#[cfg(feature = "s3")]
fn map_sdk_error<E>(
    err: &aws_smithy_runtime_api::client::result::SdkError<
        E,
        aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> BlobError
where
    E: core::fmt::Debug + aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    let status = err.raw_response().map(|r| r.status().as_u16());
    let code = err.as_service_error().and_then(|e| e.code());
    map_service_error(status, code, format!("{err}"))
}

#[cfg(feature = "s3")]
#[async_trait]
impl BlobStore for S3Store {
    async fn put(&self, key: ObjectKey, data: Bytes) -> Result<BlobId> {
        let content_type = crate::types::guess_content_type(&key);
        self.client
            .put_object()
            .bucket(self.config.bucket.as_str())
            .key(key.as_str())
            .content_type(content_type)
            .body(aws_sdk_s3::primitives::ByteStream::from(data))
            .send()
            .await
            .map_err(|e| map_sdk_error(&e))?;
        Ok(BlobId::new())
    }

    async fn get(&self, key: &ObjectKey) -> Result<Bytes> {
        let out = self
            .client
            .get_object()
            .bucket(self.config.bucket.as_str())
            .key(key.as_str())
            .send()
            .await
            .map_err(|e| map_sdk_error(&e))?;
        let data = out
            .body
            .collect()
            .await
            .map_err(|e| BlobError::Other(format!("failed to read object body: {e}")))?;
        Ok(data.into_bytes())
    }

    async fn delete(&self, key: &ObjectKey) -> Result<()> {
        self.client
            .delete_object()
            .bucket(self.config.bucket.as_str())
            .key(key.as_str())
            .send()
            .await
            .map_err(|e| map_sdk_error(&e))?;
        Ok(())
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool> {
        match self
            .client
            .head_object()
            .bucket(self.config.bucket.as_str())
            .key(key.as_str())
            .send()
            .await
        {
            Ok(_) => Ok(true),
            // HEAD responses carry no error body; rely on the 404 status.
            Err(err) if map_sdk_error(&err).is_not_found() => Ok(false),
            Err(err) => Err(map_sdk_error(&err)),
        }
    }

    async fn presigned_url(&self, key: &ObjectKey, expires: Duration) -> Result<Url> {
        let presigning = aws_sdk_s3::presigning::PresigningConfig::expires_in(expires)
            .map_err(|e| BlobError::Other(format!("invalid presigning config: {e}")))?;
        let presigned = self
            .client
            .get_object()
            .bucket(self.config.bucket.as_str())
            .key(key.as_str())
            .presigned(presigning)
            .await
            .map_err(|e| map_sdk_error(&e))?;
        Url::parse(presigned.uri())
            .map_err(|e| BlobError::Other(format!("invalid presigned URL: {e}")))
    }
}

#[cfg(not(feature = "s3"))]
#[async_trait]
impl BlobStore for S3Store {
    async fn put(&self, _key: ObjectKey, _data: Bytes) -> Result<BlobId> {
        Err(BlobError::unsupported("S3Store requires the `s3` feature"))
    }

    async fn get(&self, _key: &ObjectKey) -> Result<Bytes> {
        Err(BlobError::unsupported("S3Store requires the `s3` feature"))
    }

    async fn delete(&self, _key: &ObjectKey) -> Result<()> {
        Err(BlobError::unsupported("S3Store requires the `s3` feature"))
    }

    async fn exists(&self, _key: &ObjectKey) -> Result<bool> {
        Err(BlobError::unsupported("S3Store requires the `s3` feature"))
    }

    async fn presigned_url(&self, _key: &ObjectKey, _expires: Duration) -> Result<String> {
        Err(BlobError::unsupported("S3Store requires the `s3` feature"))
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

    fn test_config() -> S3Config {
        S3Config::new(BucketName::new("my-bucket").unwrap(), "us-east-1")
    }

    #[test]
    fn config_new_defaults() {
        let cfg = test_config();
        assert_eq!(cfg.bucket().as_str(), "my-bucket");
        assert_eq!(cfg.region(), "us-east-1");
        assert!(cfg.endpoint.is_none());
        assert!(!cfg.path_style);
        assert_eq!(cfg.credentials_mode, CredentialsMode::FromEnv);
        assert_eq!(cfg.timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn config_with_endpoint() {
        #[cfg(feature = "s3")]
        {
            let url = Url::parse("https://s3.example.com").unwrap();
            let cfg = test_config().with_endpoint(url.clone());
            assert_eq!(cfg.endpoint, Some(url));
        }
        #[cfg(not(feature = "s3"))]
        {
            let cfg = test_config().with_endpoint("https://s3.example.com");
            assert_eq!(cfg.endpoint.as_deref(), Some("https://s3.example.com"));
        }
    }

    #[test]
    fn config_builders() {
        let cfg = test_config()
            .with_path_style(true)
            .with_credentials(CredentialsMode::Static {
                access_key: "ak".into(),
                secret_key: "sk".into(),
            })
            .with_timeout(Some(Duration::from_secs(5)));
        assert!(cfg.path_style);
        assert_eq!(
            cfg.credentials_mode,
            CredentialsMode::Static {
                access_key: "ak".into(),
                secret_key: "sk".into()
            }
        );
        assert_eq!(cfg.timeout, Some(Duration::from_secs(5)));

        let cfg = cfg.with_timeout(None);
        assert_eq!(cfg.timeout, None);
    }

    #[test]
    fn error_mapping_not_found() {
        // 404 status maps to NotFound regardless of code.
        for code in [Some("NoSuchKey"), Some("NotFound"), None] {
            let err = map_service_error(Some(404), code, "missing".into());
            assert!(err.is_not_found(), "status=404 code={code:?}");
        }
        // Explicit S3 not-found codes map to NotFound regardless of status.
        for code in [Some("NoSuchKey"), Some("NotFound"), Some("NoSuchBucket")] {
            let err = map_service_error(None, code, "missing".into());
            assert!(err.is_not_found(), "code={code:?}");
        }
        // No signal at all is not NotFound.
        let err = map_service_error(None, None, "odd".into());
        assert!(!err.is_not_found());
    }

    #[test]
    fn error_mapping_permission_denied() {
        for status in [Some(401), Some(403)] {
            let err = map_service_error(status, None, "nope".into());
            assert!(matches!(err, BlobError::PermissionDenied(_)), "{status:?}");
        }
        for code in [
            Some("AccessDenied"),
            Some("InvalidAccessKeyId"),
            Some("SignatureDoesNotMatch"),
        ] {
            let err = map_service_error(None, code, "nope".into());
            assert!(matches!(err, BlobError::PermissionDenied(_)), "{code:?}");
        }
    }

    #[test]
    fn error_mapping_other() {
        let err = map_service_error(Some(500), Some("InternalError"), "boom".into());
        assert!(matches!(err, BlobError::Other(_)));
    }

    #[cfg(not(feature = "s3"))]
    #[tokio::test]
    async fn stub_returns_unsupported() {
        use crate::store::BlobStore;
        let store = S3Store::new(test_config()).await.unwrap();
        let key = ObjectKey::new("hello.txt").unwrap();
        let err = store.put(key.clone(), Bytes::from("hi")).await.unwrap_err();
        assert!(matches!(err, BlobError::Unsupported(_)));
        let err = store.get(&key).await.unwrap_err();
        assert!(matches!(err, BlobError::Unsupported(_)));
        let err = store.delete(&key).await.unwrap_err();
        assert!(matches!(err, BlobError::Unsupported(_)));
        let err = store.exists(&key).await.unwrap_err();
        assert!(matches!(err, BlobError::Unsupported(_)));
        let err = store
            .presigned_url(&key, Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(matches!(err, BlobError::Unsupported(_)));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod redaction_tests {
    use super::*;

    // REQ-BLOBKIT-101: secret keys never appear in Debug output.
    #[test]
    fn credentials_mode_debug_redacts_secret() {
        let mode = CredentialsMode::Static {
            access_key: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_key: "super-secret-value".to_string(),
        };
        let dbg = format!("{mode:?}");
        assert!(dbg.contains("[redacted]"), "secret leaked: {dbg}");
        assert!(!dbg.contains("super-secret-value"), "secret leaked: {dbg}");
    }

    #[test]
    fn s3_config_debug_has_no_secret_field() {
        let cfg = S3Config {
            bucket: BucketName::new("my-bucket".to_string())
                .expect("INVARIANT: 'my-bucket' is a valid S3 bucket name"),
            region: "us-east-1".to_string(),
            endpoint: None,
            path_style: true,
            credentials_mode: CredentialsMode::Static {
                access_key: "AKIAIOSFODNN7EXAMPLE".to_string(),
                secret_key: "super-secret-value".to_string(),
            },
            timeout: None,
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("super-secret-value"), "secret leaked: {dbg}");
    }
}
