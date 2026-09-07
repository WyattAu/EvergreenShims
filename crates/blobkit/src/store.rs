//! The unified [`BlobStore`] trait.

extern crate alloc;

use alloc::boxed::Box;
use core::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;

#[cfg(not(feature = "s3"))]
use crate::error::BlobError;
use crate::error::Result;
use crate::types::{BlobId, ObjectKey};

/// Unified async blob storage interface.
///
/// Implementors provide the five core operations. The trait is object-safe so
/// it can be used as `dyn BlobStore` and boxed across service boundaries
/// (e.g. in archival-shim, backup-shim, kestrel-storage, ferro).
///
/// # Example
///
/// ```rust,no_run
/// use blobkit::store::BlobStore;
/// use blobkit::types::ObjectKey;
/// use bytes::Bytes;
///
/// async fn example<S: BlobStore>(store: &S) -> Result<(), blobkit::error::BlobError> {
///     let key = ObjectKey::new("hello.txt")?;
///     let id = store.put(key.clone(), Bytes::from("hello world")).await?;
///     let data = store.get(&key).await?;
///     assert_eq!(data, Bytes::from("hello world"));
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Store `data` at `key`, overwriting any existing value.
    ///
    /// Returns a [`BlobId`] that uniquely identifies this version of the
    /// object. The id is freshly generated on each `put` even when overwriting.
    ///
    /// # Requirements
    /// REQ-BK-001, REQ-BK-003, REQ-BK-006
    async fn put(&self, key: ObjectKey, data: Bytes) -> Result<BlobId>;

    /// Retrieve the bytes at `key`.
    ///
    /// # Errors
    /// Returns [`BlobError::NotFound`] if the key does not exist.
    ///
    /// # Requirements
    /// REQ-BK-001, REQ-BK-103
    async fn get(&self, key: &ObjectKey) -> Result<Bytes>;

    /// Delete the object at `key`.
    ///
    /// # Errors
    /// Returns [`BlobError::NotFound`] if the key does not exist.
    ///
    /// # Requirements
    /// REQ-BK-002, REQ-BK-103
    async fn delete(&self, key: &ObjectKey) -> Result<()>;

    /// Returns `true` if `key` exists.
    async fn exists(&self, key: &ObjectKey) -> Result<bool>;

    /// Generate a pre-signed URL that can be used to download `key` without
    /// authentication. The URL should expire after `expires`.
    ///
    /// Backends that do not support pre-signed URLs must return
    /// [`BlobError::Unsupported`].
    #[cfg(feature = "s3")]
    async fn presigned_url(&self, key: &ObjectKey, expires: Duration) -> Result<url::Url>;

    /// Generate a pre-signed URL (fallback when the `s3` / `url` feature is
    /// not enabled). The default implementation returns `Unsupported`.
    #[cfg(not(feature = "s3"))]
    async fn presigned_url(
        &self,
        _key: &ObjectKey,
        _expires: Duration,
    ) -> Result<alloc::string::String> {
        Err(BlobError::unsupported(
            "presigned_url requires the `s3` feature",
        ))
    }
}

/// Convenience extension: `Box<dyn BlobStore>` forwarding.
///
/// Allows `Box<S>` where `S: BlobStore` to itself implement `BlobStore` so a
/// boxed trait object can be used uniformly.
#[async_trait]
impl<S> BlobStore for alloc::boxed::Box<S>
where
    S: BlobStore + ?Sized,
{
    async fn put(&self, key: ObjectKey, data: Bytes) -> Result<BlobId> {
        (**self).put(key, data).await
    }

    async fn get(&self, key: &ObjectKey) -> Result<Bytes> {
        (**self).get(key).await
    }

    async fn delete(&self, key: &ObjectKey) -> Result<()> {
        (**self).delete(key).await
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool> {
        (**self).exists(key).await
    }

    #[cfg(feature = "s3")]
    async fn presigned_url(&self, key: &ObjectKey, expires: Duration) -> Result<url::Url> {
        (**self).presigned_url(key, expires).await
    }

    #[cfg(not(feature = "s3"))]
    async fn presigned_url(
        &self,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<alloc::string::String> {
        (**self).presigned_url(key, expires).await
    }
}
