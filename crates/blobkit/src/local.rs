//! Local filesystem blob store.
//!
//! Each object is a file under `root / key`. Writes are atomic via
//! `tempfile` + `rename`.

extern crate alloc;

use alloc::boxed::Box;
#[cfg(feature = "std")]
use alloc::string::String;
use async_trait::async_trait;

#[cfg(feature = "std")]
use bytes::Bytes;
#[cfg(feature = "std")]
use core::time::Duration;
#[cfg(feature = "std")]
use std::path::{Path, PathBuf};

#[cfg(feature = "std")]
use crate::error::{BlobError, Result};
#[cfg(feature = "std")]
use crate::store::BlobStore;
#[cfg(feature = "std")]
use crate::types::{BlobId, BlobMetadata, ObjectKey};

#[cfg(all(feature = "std", feature = "tracing"))]
use tracing::{debug, trace};

// ---------------------------------------------------------------------------
// Std implementation
// ---------------------------------------------------------------------------

/// Filesystem-backed blob store.
///
/// ```rust,no_run
/// use blobkit::local::LocalStore;
/// use blobkit::store::BlobStore;
/// use blobkit::types::ObjectKey;
/// use bytes::Bytes;
/// use std::path::PathBuf;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), blobkit::error::BlobError> {
/// let store = LocalStore::new(PathBuf::from("/tmp/blobs")).await?;
/// let key = ObjectKey::new("hello.txt")?;
/// store.put(key.clone(), Bytes::from("hello")).await?;
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "std")]
#[derive(Debug)]
pub struct LocalStore {
    root: PathBuf,
    max_bytes: Option<u64>,
}

#[cfg(feature = "std")]
impl LocalStore {
    /// Create a new store rooted at `root`. The directory is created if it
    /// does not exist.
    ///
    /// # Errors
    /// Returns `Io` if the directory cannot be created or is not a directory.
    pub async fn new(root: PathBuf) -> Result<Self> {
        Self::with_limits(root, None).await
    }

    /// Create a new store with an optional maximum object size.
    ///
    /// `max_bytes == Some(n)` causes `put` to reject any payload larger than
    /// `n` bytes with [`BlobError::StorageFull`] before touching the
    /// filesystem — a guard against OOM when callers accidentally attempt to
    /// store unbounded data.
    pub async fn with_limits(root: PathBuf, max_bytes: Option<u64>) -> Result<Self> {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(BlobError::from)?;
        let meta = tokio::fs::metadata(&root).await.map_err(BlobError::from)?;
        if !meta.is_dir() {
            return Err(BlobError::Other(format!(
                "not a directory: {}",
                root.display()
            )));
        }
        Ok(Self { root, max_bytes })
    }

    /// Create without touching the filesystem. Useful when the caller has
    /// already ensured `root` exists (e.g. in tests that manage tempdirs).
    #[must_use]
    pub fn new_unchecked(root: PathBuf) -> Self {
        Self {
            root,
            max_bytes: None,
        }
    }

    /// Set or clear the maximum allowed object size.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: Option<u64>) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// The root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve `key` to an absolute filesystem path, ensuring the result
    /// remains under `root` (defense against `..` even though `ObjectKey`
    /// already validates).
    fn resolve(&self, key: &ObjectKey) -> Result<PathBuf> {
        let path = self.root.join(key.as_str());
        if key.as_str().contains("..") {
            return Err(BlobError::invalid_key("key contains '..'"));
        }
        Ok(path)
    }

    /// Ensure the parent directory for `path` exists.
    async fn ensure_parent(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(BlobError::from)?;
        }
        Ok(())
    }

    /// Guess content type for a key.
    fn content_type_for(key: &ObjectKey) -> String {
        crate::types::guess_content_type(key)
    }
}

#[cfg(feature = "std")]
#[async_trait]
impl BlobStore for LocalStore {
    async fn put(&self, key: ObjectKey, data: Bytes) -> Result<BlobId> {
        #[cfg(feature = "tracing")]
        trace!(key = %key, size = data.len(), "local put");

        if let Some(limit) = self.max_bytes {
            if data.len() as u64 > limit {
                return Err(BlobError::StorageFull);
            }
        }

        let dest = self.resolve(&key)?;
        self.ensure_parent(&dest).await?;

        let parent = dest.parent().unwrap_or(&self.root);

        let mut tmp = tempfile::Builder::new()
            .prefix(".blobkit-")
            .tempfile_in(parent)
            .map_err(|e| BlobError::Other(format!("tempfile create: {e}")))?;

        {
            use std::io::Write;
            tmp.write_all(&data)
                .map_err(|e| BlobError::Other(format!("tempfile write: {e}")))?;
            tmp.flush()
                .map_err(|e| BlobError::Other(format!("tempfile flush: {e}")))?;
            tmp.as_file()
                .sync_all()
                .map_err(|e| BlobError::Other(format!("sync: {e}")))?;
        }

        tmp.persist(&dest).map_err(|e| BlobError::from(e.error))?;

        let _meta = BlobMetadata::new(key.clone(), data.len() as u64)
            .with_content_type(Self::content_type_for(&key));

        #[cfg(feature = "sha2")]
        {
            let _ = crate::types::compute_sha256(&data);
        }

        Ok(BlobId::new())
    }

    async fn get(&self, key: &ObjectKey) -> Result<Bytes> {
        #[cfg(feature = "tracing")]
        trace!(key = %key, "local get");

        let path = self.resolve(key)?;
        match tokio::fs::read(&path).await {
            Ok(v) => Ok(Bytes::from(v)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(BlobError::not_found(key.as_str()))
            }
            Err(e) => Err(BlobError::from(e)),
        }
    }

    async fn delete(&self, key: &ObjectKey) -> Result<()> {
        #[cfg(feature = "tracing")]
        debug!(key = %key, "local delete");

        let path = self.resolve(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(BlobError::not_found(key.as_str()))
            }
            Err(e) => Err(BlobError::from(e)),
        }
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool> {
        let path = self.resolve(key)?;
        match tokio::fs::metadata(&path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(BlobError::from(e)),
        }
    }

    #[cfg(feature = "s3")]
    async fn presigned_url(&self, key: &ObjectKey, expires: Duration) -> Result<url::Url> {
        let path = self.resolve(key)?;
        if !self.exists(key).await? {
            return Err(BlobError::not_found(key.as_str()));
        }
        let mut url = url::Url::from_file_path(&path)
            .map_err(|_| BlobError::Other("failed to construct file URL".into()))?;
        url.query_pairs_mut()
            .append_pair("expires", &expires.as_secs().to_string());
        Ok(url)
    }

    #[cfg(not(feature = "s3"))]
    async fn presigned_url(&self, key: &ObjectKey, expires: Duration) -> Result<String> {
        if !self.exists(key).await? {
            return Err(BlobError::not_found(key.as_str()));
        }
        let path = self.resolve(key)?;
        Ok(format!(
            "file://{}?expires={}",
            path.display(),
            expires.as_secs()
        ))
    }
}

// ---------------------------------------------------------------------------
// No-std stub: LocalStore is unavailable without `std`.
// ---------------------------------------------------------------------------

/// Stub when `std` is disabled.
#[cfg(not(feature = "std"))]
#[derive(Debug)]
pub struct LocalStore(());

#[cfg(not(feature = "std"))]
impl LocalStore {
    /// Always returns unsupported when `std` is disabled.
    pub fn new_unchecked(_root: alloc::string::String) -> Self {
        Self(())
    }
}

#[cfg(not(feature = "std"))]
#[async_trait]
impl crate::store::BlobStore for LocalStore {
    async fn put(
        &self,
        _key: crate::types::ObjectKey,
        _data: bytes::Bytes,
    ) -> crate::error::Result<crate::types::BlobId> {
        Err(crate::error::BlobError::unsupported(
            "LocalStore requires std feature",
        ))
    }

    async fn get(&self, _key: &crate::types::ObjectKey) -> crate::error::Result<bytes::Bytes> {
        Err(crate::error::BlobError::unsupported(
            "LocalStore requires std feature",
        ))
    }

    async fn delete(&self, _key: &crate::types::ObjectKey) -> crate::error::Result<()> {
        Err(crate::error::BlobError::unsupported(
            "LocalStore requires std feature",
        ))
    }

    async fn exists(&self, _key: &crate::types::ObjectKey) -> crate::error::Result<bool> {
        Err(crate::error::BlobError::unsupported(
            "LocalStore requires std feature",
        ))
    }

    #[cfg(feature = "s3")]
    async fn presigned_url(
        &self,
        _key: &crate::types::ObjectKey,
        _expires: core::time::Duration,
    ) -> crate::error::Result<url::Url> {
        Err(crate::error::BlobError::unsupported(
            "LocalStore requires std feature",
        ))
    }

    #[cfg(not(feature = "s3"))]
    async fn presigned_url(
        &self,
        _key: &crate::types::ObjectKey,
        _expires: core::time::Duration,
    ) -> crate::error::Result<alloc::string::String> {
        Err(crate::error::BlobError::unsupported(
            "LocalStore requires std feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// Tests exercise failure paths and invariants directly; unwrap/expect,
// slicing, and panicking asserts are acceptable here — violations
// surface as test failures, not production panics.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::store::BlobStore;
    use tempfile::TempDir;

    async fn temp_store() -> (TempDir, LocalStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::new(dir.path().to_path_buf()).await.unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn put_and_get() {
        let (_dir, store) = temp_store().await;
        let key = ObjectKey::new("a/b.txt").unwrap();
        store.put(key.clone(), Bytes::from("hello")).await.unwrap();
        let got = store.get(&key).await.unwrap();
        assert_eq!(got, Bytes::from("hello"));
    }

    #[tokio::test]
    async fn put_creates_parent_dirs() {
        let (_dir, store) = temp_store().await;
        let key = ObjectKey::new("deep/nested/file.bin").unwrap();
        store
            .put(key.clone(), Bytes::from(vec![1, 2, 3]))
            .await
            .unwrap();
        assert!(store.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn overwrite() {
        let (_dir, store) = temp_store().await;
        let key = ObjectKey::new("file.txt").unwrap();
        store.put(key.clone(), Bytes::from("v1")).await.unwrap();
        store.put(key.clone(), Bytes::from("v2")).await.unwrap();
        assert_eq!(store.get(&key).await.unwrap(), Bytes::from("v2"));
    }

    #[tokio::test]
    async fn not_found() {
        let (_dir, store) = temp_store().await;
        let key = ObjectKey::new("missing.txt").unwrap();
        assert!(store.get(&key).await.unwrap_err().is_not_found());
        assert!(store.delete(&key).await.unwrap_err().is_not_found());
        assert!(!store.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn delete() {
        let (_dir, store) = temp_store().await;
        let key = ObjectKey::new("del.txt").unwrap();
        store.put(key.clone(), Bytes::from("x")).await.unwrap();
        store.delete(&key).await.unwrap();
        assert!(!store.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn max_bytes_guard() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::with_limits(dir.path().to_path_buf(), Some(5))
            .await
            .unwrap();
        let key = ObjectKey::new("big.bin").unwrap();
        let err = store
            .put(key.clone(), Bytes::from(vec![0u8; 6]))
            .await
            .unwrap_err();
        assert!(matches!(err, BlobError::StorageFull));
        store
            .put(key.clone(), Bytes::from(vec![0u8; 5]))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn atomic_write_no_partial_on_overwrite() {
        let (_dir, store) = temp_store().await;
        let key = ObjectKey::new("atomic.txt").unwrap();
        store.put(key.clone(), Bytes::from("first")).await.unwrap();
        store.put(key.clone(), Bytes::from("second")).await.unwrap();
        assert_eq!(store.get(&key).await.unwrap(), Bytes::from("second"));
    }

    #[tokio::test]
    async fn presigned_url_exists_check() {
        let (_dir, store) = temp_store().await;
        let key = ObjectKey::new("presign.txt").unwrap();
        let err = store
            .presigned_url(&key, Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(err.is_not_found());
        store.put(key.clone(), Bytes::from("data")).await.unwrap();
        let url = store
            .presigned_url(&key, Duration::from_secs(60))
            .await
            .unwrap();
        #[cfg(feature = "s3")]
        assert!(url.to_string().contains("expires"));
        #[cfg(not(feature = "s3"))]
        assert!(url.contains("expires"));
    }
}
