//! In-memory blob store backed by [`DashMap`].
//!
//! Suitable for tests and ephemeral workloads. All data is stored in-process
//! and dropped when the store is dropped.

extern crate alloc;

use alloc::boxed::Box;
#[cfg(not(feature = "s3"))]
use alloc::string::String;
use alloc::vec::Vec;
use async_trait::async_trait;
use bytes::Bytes;
use core::time::Duration;

use crate::error::{BlobError, Result};
use crate::store::BlobStore;
use crate::types::{BlobId, BlobMetadata, ObjectKey};

#[cfg(feature = "memory")]
use dashmap::DashMap;

#[cfg(all(feature = "memory", feature = "tracing"))]
use tracing::{debug, trace};

/// In-memory blob store.
///
/// Backed by a concurrent hash map so it can be shared across tasks via
/// `Arc<MemoryStore>` without additional locking.
///
/// # Example
///
/// ```rust
/// use blobkit::memory::MemoryStore;
/// use blobkit::store::BlobStore;
/// use blobkit::types::ObjectKey;
/// use bytes::Bytes;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), blobkit::error::BlobError> {
/// let store = MemoryStore::new();
/// let key = ObjectKey::new("hello.txt")?;
/// store.put(key.clone(), Bytes::from("hello")).await?;
/// let data = store.get(&key).await?;
/// assert_eq!(data, Bytes::from("hello"));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct MemoryStore {
    #[cfg(all(feature = "memory", feature = "std"))]
    inner: DashMap<ObjectKey, Entry>,
    #[cfg(all(not(feature = "memory"), feature = "std"))]
    inner: std::sync::Mutex<std::collections::HashMap<ObjectKey, Entry>>,
    #[cfg(not(feature = "std"))]
    _no_std: (),
}

/// Internal entry storing bytes plus metadata.
#[derive(Debug, Clone)]
#[cfg(feature = "std")]
struct Entry {
    data: Bytes,
    meta: BlobMetadata,
}

impl MemoryStore {
    /// Create a new empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            #[cfg(all(feature = "memory", feature = "std"))]
            inner: DashMap::new(),
            #[cfg(all(not(feature = "memory"), feature = "std"))]
            inner: std::sync::Mutex::new(std::collections::HashMap::new()),
            #[cfg(not(feature = "std"))]
            _no_std: (),
        }
    }

    /// Number of objects currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        #[cfg(all(feature = "memory", feature = "std"))]
        {
            self.inner.len()
        }
        #[cfg(all(not(feature = "memory"), feature = "std"))]
        {
            self.inner
                .lock()
                .map(|g: &std::collections::HashMap<ObjectKey, Entry>| g.len())
                .unwrap_or(0)
        }
        #[cfg(not(feature = "std"))]
        {
            0
        }
    }

    /// Returns `true` if the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all objects.
    pub fn clear(&self) {
        #[cfg(all(feature = "memory", feature = "std"))]
        self.inner.clear();
        #[cfg(all(not(feature = "memory"), feature = "std"))]
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
        #[cfg(not(feature = "std"))]
        {}
    }

    /// Get metadata for `key`, if present.
    #[must_use]
    pub fn metadata(&self, key: &ObjectKey) -> Option<BlobMetadata> {
        #[cfg(all(feature = "memory", feature = "std"))]
        {
            self.inner.get(key).map(|e| e.meta.clone())
        }
        #[cfg(all(not(feature = "memory"), feature = "std"))]
        {
            self.inner
                .lock()
                .ok()
                .and_then(|g: &std::collections::HashMap<ObjectKey, Entry>| {
                    g.get(key).map(|e| e.meta.clone())
                })
        }
        #[cfg(not(feature = "std"))]
        {
            let _ = key;
            None
        }
    }

    /// List all keys.
    #[must_use]
    pub fn list_keys(&self) -> Vec<ObjectKey> {
        #[cfg(all(feature = "memory", feature = "std"))]
        {
            self.inner.iter().map(|kv| kv.key().clone()).collect()
        }
        #[cfg(all(not(feature = "memory"), feature = "std"))]
        {
            self.inner
                .lock()
                .map(|g: &std::collections::HashMap<ObjectKey, Entry>| {
                    g.keys().cloned().collect::<Vec<_>>()
                })
                .unwrap_or_default()
        }
        #[cfg(not(feature = "std"))]
        {
            Vec::new()
        }
    }
}

#[async_trait]
impl BlobStore for MemoryStore {
    async fn put(&self, key: ObjectKey, data: Bytes) -> Result<BlobId> {
        #[cfg(not(feature = "std"))]
        {
            let _ = (key, data);
            return Err(BlobError::unsupported("MemoryStore requires std feature"));
        }
        #[cfg(feature = "std")]
        {
            #[cfg(all(feature = "memory", feature = "tracing"))]
            trace!(key = %key, size = data.len(), "memory put");

            let size = data.len() as u64;
            let content_type = crate::types::guess_content_type(&key);
            #[cfg(feature = "sha2")]
            let meta = {
                let mut m =
                    BlobMetadata::new(key.clone(), size).with_content_type(content_type.clone());
                let digest = crate::types::compute_sha256(&data);
                m.sha256 = Some(digest);
                m
            };
            #[cfg(not(feature = "sha2"))]
            let meta = BlobMetadata::new(key.clone(), size).with_content_type(content_type);

            let id = BlobId::new();
            let entry = Entry { data, meta };

            #[cfg(all(feature = "memory", feature = "std"))]
            {
                self.inner.insert(key, entry);
            }
            #[cfg(all(not(feature = "memory"), feature = "std"))]
            {
                let mut g = self
                    .inner
                    .lock()
                    .map_err(|_| BlobError::Other("memory store lock poisoned".into()))?;
                g.insert(key, entry);
            }

            Ok(id)
        }
    }

    async fn get(&self, key: &ObjectKey) -> Result<Bytes> {
        #[cfg(not(feature = "std"))]
        {
            let _ = key;
            return Err(BlobError::unsupported("MemoryStore requires std feature"));
        }
        #[cfg(feature = "std")]
        {
            #[cfg(all(feature = "memory", feature = "tracing"))]
            trace!(key = %key, "memory get");

            #[cfg(all(feature = "memory", feature = "std"))]
            {
                return self
                    .inner
                    .get(key)
                    .map(|e| e.data.clone())
                    .ok_or_else(|| BlobError::not_found(key.as_str()));
            }
            #[cfg(all(not(feature = "memory"), feature = "std"))]
            {
                let g = self
                    .inner
                    .lock()
                    .map_err(|_| BlobError::Other("memory store lock poisoned".into()))?;
                return g
                    .get(key)
                    .map(|e| e.data.clone())
                    .ok_or_else(|| BlobError::not_found(key.as_str()));
            }
            #[allow(unreachable_code)]
            {
                Err(BlobError::unsupported("unsupported"))
            }
        }
    }

    async fn delete(&self, key: &ObjectKey) -> Result<()> {
        #[cfg(not(feature = "std"))]
        {
            let _ = key;
            return Err(BlobError::unsupported("MemoryStore requires std feature"));
        }
        #[cfg(feature = "std")]
        {
            #[cfg(all(feature = "memory", feature = "tracing"))]
            debug!(key = %key, "memory delete");

            let removed = {
                #[cfg(all(feature = "memory", feature = "std"))]
                {
                    self.inner.remove(key).is_some()
                }
                #[cfg(all(not(feature = "memory"), feature = "std"))]
                {
                    let mut g = self
                        .inner
                        .lock()
                        .map_err(|_| BlobError::Other("memory store lock poisoned".into()))?;
                    g.remove(key).is_some()
                }
                #[cfg(not(feature = "std"))]
                {
                    false
                }
            };

            if removed {
                Ok(())
            } else {
                Err(BlobError::not_found(key.as_str()))
            }
        }
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool> {
        #[cfg(not(feature = "std"))]
        {
            let _ = key;
            return Err(BlobError::unsupported("MemoryStore requires std feature"));
        }
        #[cfg(feature = "std")]
        {
            #[cfg(all(feature = "memory", feature = "std"))]
            {
                return Ok(self.inner.contains_key(key));
            }
            #[cfg(all(not(feature = "memory"), feature = "std"))]
            {
                let g = self
                    .inner
                    .lock()
                    .map_err(|_| BlobError::Other("memory store lock poisoned".into()))?;
                return Ok(g.contains_key(key));
            }
            #[allow(unreachable_code)]
            {
                let _ = key;
                Ok(false)
            }
        }
    }

    #[cfg(feature = "s3")]
    async fn presigned_url(&self, _key: &ObjectKey, _expires: Duration) -> Result<url::Url> {
        Err(BlobError::unsupported(
            "presigned_url is not supported for MemoryStore",
        ))
    }

    #[cfg(not(feature = "s3"))]
    async fn presigned_url(&self, _key: &ObjectKey, _expires: Duration) -> Result<String> {
        Err(BlobError::unsupported(
            "presigned_url is not supported for MemoryStore (enable `s3` feature for Url support)",
        ))
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
    use crate::store::BlobStore;

    #[tokio::test]
    async fn put_and_get() {
        let store = MemoryStore::new();
        let key = ObjectKey::new("a/b.txt").unwrap();
        let data = Bytes::from("hello");
        let id = store.put(key.clone(), data.clone()).await.unwrap();
        assert!(!id.to_string().is_empty());
        let got = store.get(&key).await.unwrap();
        assert_eq!(got, data);
    }

    #[tokio::test]
    async fn overwrite() {
        let store = MemoryStore::new();
        let key = ObjectKey::new("file.txt").unwrap();
        store.put(key.clone(), Bytes::from("v1")).await.unwrap();
        store.put(key.clone(), Bytes::from("v2")).await.unwrap();
        assert_eq!(store.get(&key).await.unwrap(), Bytes::from("v2"));
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn not_found() {
        let store = MemoryStore::new();
        let key = ObjectKey::new("missing.txt").unwrap();
        let err = store.get(&key).await.unwrap_err();
        assert!(err.is_not_found());
        let err = store.delete(&key).await.unwrap_err();
        assert!(err.is_not_found());
    }

    #[tokio::test]
    async fn delete_and_exists() {
        let store = MemoryStore::new();
        let key = ObjectKey::new("del.txt").unwrap();
        assert!(!store.exists(&key).await.unwrap());
        store.put(key.clone(), Bytes::from("x")).await.unwrap();
        assert!(store.exists(&key).await.unwrap());
        store.delete(&key).await.unwrap();
        assert!(!store.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn clear() {
        let store = MemoryStore::new();
        store
            .put(ObjectKey::new("a").unwrap(), Bytes::from("1"))
            .await
            .unwrap();
        store
            .put(ObjectKey::new("b").unwrap(), Bytes::from("2"))
            .await
            .unwrap();
        assert_eq!(store.len(), 2);
        store.clear();
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn metadata() {
        let store = MemoryStore::new();
        let key = ObjectKey::new("meta.txt").unwrap();
        store.put(key.clone(), Bytes::from("abc")).await.unwrap();
        let meta = store.metadata(&key).unwrap();
        assert_eq!(meta.key, key);
        assert_eq!(meta.size, 3);
        assert!(!meta.content_type.is_empty());
    }

    #[tokio::test]
    async fn presigned_url_unsupported() {
        let store = MemoryStore::new();
        let key = ObjectKey::new("a.txt").unwrap();
        let res = store.presigned_url(&key, Duration::from_secs(60)).await;
        assert!(res.is_err());
    }
}
