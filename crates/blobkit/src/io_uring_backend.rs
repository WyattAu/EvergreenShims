//! io_uring-backed local filesystem blob store (Linux only).
//!
//! **Feature:** `io-uring` (implies `std`). **Target:** `#[cfg(target_os =
//! "linux")]`, kernel ≥ 5.1.
//!
//! This crate carries `#![forbid(unsafe_code)]` everywhere except here: the
//! `io-uring` crate's submission queue is an unsafe ring shared with the
//! kernel. Every `unsafe` block below is annotated with the invariant it
//! upholds.
//!
//! # What is here
//!
//! - [`IoUringFile`]: a minimal correct file backend using raw io_uring
//!   SQEs: chunked, batched `Write`/`Read` operations with a
//!   submit → wait → reap → resubmit loop that handles short reads/writes.
//! - [`IoUringStore`]: a [`BlobStore`](crate::store::BlobStore) over
//!   `IoUringFile` scoped to `put`/`get` on local files, with the same
//!   atomic-write semantics as [`LocalStore`](crate::local::LocalStore)
//!   (tempfile + fsync + rename).
//!
//! # Honest scope notes (read before using)
//!
//! - **The async trait is a veneer here.** The ring waits
//!   (`submit_and_wait`) block the calling thread. The [`store::IoUringStore`]
//!   methods therefore run the ring sections inside
//!   `tokio::task::spawn_blocking`, so `BlobStore` callers never block the
//!   async runtime. The raw [`IoUringFile`] API remains synchronous by
//!   design: it is intended for dedicated I/O threads. A fully async
//!   integration needs registered buffers, submission coalescing across
//!   tasks, and a completion dispatcher — deliberately out of scope for
//!   this minimal version.
//! - **fsync ordering goes through `std`.** `put` issues `file.sync_all()`
//!   (and `rename` via `tempfile::persist`) rather than the io_uring
//!   `FSYNC`/`RENAMAT2` opcodes: io_uring fsync historically drained only
//!   ops issued through the same ring and had ordering bugs on older
//!   kernels, and rename is only available on very recent kernels. The
//!   durability contract is identical to `LocalStore`; only the bulk data
//!   transfer uses io_uring.
//! - **One ring per operation.** Ring setup is a handful of syscalls
//!   (~microseconds). Amortizing one ring per worker thread is the obvious
//!   next step and is intentionally left out of this minimal version.
//!
//! # Benchmark
//!
//! `benches/io_uring_bench.rs` (built only with the feature) compares
//! sequential 1 MiB writes/reads against `tokio::fs` and `LocalStore`.

#[cfg(all(target_os = "linux", feature = "io-uring"))]
pub mod backend {
    //! Raw io_uring file primitives ([`IoUringFile`]): chunked, batched
    //! submit → wait → reap I/O with short read/write resubmission.
    //!
    //! Tests exercise failure paths directly; unwrap/expect, slicing, and
    //! panicking asserts are acceptable here — violations surface as test
    //! failures, not production panics.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use std::collections::{HashMap, VecDeque};
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    use io_uring::{opcode, types, IoUring};

    /// Default ring depth (in-flight SQEs per submission batch).
    pub const DEFAULT_RING_SIZE: u32 = 64;
    /// Default chunk size per SQE (128 KiB).
    pub const DEFAULT_CHUNK_SIZE: usize = 128 * 1024;

    /// A file operated through raw io_uring SQEs.
    ///
    /// Chunked batched I/O with a resubmission loop for short reads/writes.
    /// One ring per file — see the module docs for why this is acceptable
    /// for the scoped use cases.
    pub struct IoUringFile {
        file: File,
        ring: IoUring,
        ring_size: u32,
        chunk_size: usize,
    }

    /// Remaining work for the submit/reap loop.
    struct Piece {
        file_off: u64,
        buf_off: usize,
        len: usize,
    }

    impl IoUringFile {
        /// Create (or truncate) `path` for writing.
        ///
        /// # Errors
        /// Returns the raw `io::Error` from open or ring creation.
        pub fn create(path: &Path) -> io::Result<Self> {
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?;
            Self::from_file(file)
        }

        /// Open `path` for reading.
        ///
        /// # Errors
        /// Returns the raw `io::Error` from open or ring creation.
        pub fn open(path: &Path) -> io::Result<Self> {
            let file = File::open(path)?;
            Self::from_file(file)
        }

        /// Wrap an already-open `File` (any valid fd: the ring issues ops
        /// against the file description, so a `try_clone` dup of a tempfile
        /// handle works).
        ///
        /// # Errors
        /// Returns the raw `io::Error` from ring creation.
        pub fn from_file(file: File) -> io::Result<Self> {
            let ring = IoUring::new(DEFAULT_RING_SIZE)?;
            Ok(Self {
                file,
                ring,
                ring_size: DEFAULT_RING_SIZE,
                chunk_size: DEFAULT_CHUNK_SIZE,
            })
        }

        /// The wrapped file handle (for `sync_all` and friends).
        #[must_use]
        pub fn file(&self) -> &File {
            &self.file
        }

        /// Write all of `data` at `offset`, via batched io_uring writes.
        ///
        /// Handles short writes by resubmitting the remainder. Blocks until
        /// every byte is confirmed by a completion event.
        ///
        /// # Errors
        /// `io::Error` from submission, completion (`-errno`), or a
        /// zero-length write (no forward progress possible).
        pub fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
            let mut work: VecDeque<Piece> = data
                .chunks(self.chunk_size)
                .scan(offset, |off, chunk| {
                    let p = Piece {
                        file_off: *off,
                        buf_off: chunk.as_ptr() as usize - data.as_ptr() as usize,
                        len: chunk.len(),
                    };
                    *off += chunk.len() as u64;
                    Some(p)
                })
                .collect();
            self.drive(true, data, &mut work)
        }

        /// Read exactly `buf.len()` bytes into `buf` at `offset`, via
        /// batched io_uring reads. Handles short reads by resubmitting.
        ///
        /// # Errors
        /// `io::Error` from submission/completion, or `UnexpectedEof` if the
        /// file ends before `buf` is full.
        pub fn read_all_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
            let base = buf.as_ptr() as usize;
            let mut work: VecDeque<Piece> = buf
                .chunks_mut(self.chunk_size)
                .scan(offset, |off, chunk| {
                    let p = Piece {
                        file_off: *off,
                        buf_off: chunk.as_ptr() as usize - base,
                        len: chunk.len(),
                    };
                    *off += chunk.len() as u64;
                    Some(p)
                })
                .collect();
            self.drive(false, buf, &mut work)
        }

        /// Core submit → wait → reap → resubmit loop.
        ///
        /// SAFETY (the one invariant the `unsafe` blocks rely on): every
        /// SQE pushed references memory inside `buf`, and `buf` outlives the
        /// loop — completions for all in-flight operations are reaped before
        /// this function returns, and the kernel holds no references after
        /// its completion event.
        fn drive(&mut self, write: bool, buf: &[u8], work: &mut VecDeque<Piece>) -> io::Result<()> {
            let fd = types::Fd(self.file.as_raw_fd());
            let mut in_flight: HashMap<u64, Piece> = HashMap::new();
            let mut next_id: u64 = 0;

            loop {
                // Fill the submission queue up to the ring depth.
                while in_flight.len() < self.ring_size as usize {
                    let Some(piece) = work.pop_front() else {
                        break;
                    };
                    let sqe = if write {
                        // SAFETY: see `drive` doc comment. The pointer is
                        // derived from `buf` and remains valid until the
                        // completion is reaped below.
                        let ptr = unsafe { buf.as_ptr().add(piece.buf_off) };
                        opcode::Write::new(fd, ptr, piece.len as u32)
                            .offset(piece.file_off)
                            .build()
                            .user_data(next_id)
                    } else {
                        // SAFETY: see `drive` doc comment. Mutable aliasing
                        // is impossible: pieces are disjoint by
                        // construction, and each piece is in flight at most
                        // once.
                        let ptr = unsafe { buf.as_ptr().add(piece.buf_off) } as *mut u8;
                        opcode::Read::new(fd, ptr, piece.len as u32)
                            .offset(piece.file_off)
                            .build()
                            .user_data(next_id)
                    };
                    // SQ capacity equals `ring_size` and we cap in-flight at
                    // the same, so push can only fail on kernel-side
                    // overflow; treat that as a transient retry.
                    //
                    // SAFETY: `ring` is not shared with any other thread
                    // (`IoUringFile` is not `Sync`), and the queue has spare
                    // capacity by the in-flight cap above, so the SQ tail we
                    // publish is exclusively ours.
                    let pushed = unsafe { self.ring.submission().push(&sqe) };
                    match pushed {
                        Ok(()) => {
                            in_flight.insert(next_id, piece);
                            next_id += 1;
                        }
                        Err(_entry) => {
                            work.push_front(piece);
                            break;
                        }
                    }
                }

                if in_flight.is_empty() {
                    return Ok(());
                }

                // Block until at least one completion lands, then drain.
                self.ring.submit_and_wait(1)?;
                while let Some(cqe) = self.ring.completion().next() {
                    let mut piece = in_flight
                        .remove(&cqe.user_data())
                        .ok_or_else(|| io::Error::other("io_uring: unknown user_data"))?;
                    let n = cqe.result();
                    if n < 0 {
                        return Err(io::Error::from_raw_os_error(-n));
                    }
                    let n = n as usize;
                    if n == piece.len {
                        continue;
                    }
                    if n == 0 {
                        // Writes must make progress; reads hit EOF.
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "io_uring: zero-length completion",
                        ));
                    }
                    // Short read/write: resubmit the remainder.
                    piece.file_off += n as u64;
                    piece.buf_off += n;
                    piece.len -= n;
                    work.push_back(piece);
                }
            }
        }
    }

    #[cfg(all(test, feature = "io-uring"))]
    mod tests {
        use super::*;

        #[test]
        fn write_then_read_roundtrip() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("f.bin");
            let payload: Vec<u8> = (0..3 * DEFAULT_CHUNK_SIZE as u64)
                .map(|i| (i % 251) as u8)
                .collect();
            {
                let mut f = IoUringFile::create(&path).unwrap();
                f.write_all_at(0, &payload).unwrap();
                f.file().sync_all().unwrap();
            }
            let mut out = vec![0u8; payload.len()];
            let mut f = IoUringFile::open(&path).unwrap();
            f.read_all_at(0, &mut out).unwrap();
            assert_eq!(out, payload);
        }

        #[test]
        fn offset_write_extends_middle() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("f.bin");
            {
                let mut f = IoUringFile::create(&path).unwrap();
                f.write_all_at(0, &[0u8; 4096]).unwrap();
                f.write_all_at(1000, &[7u8; 16]).unwrap();
            }
            // `create` opens write-only; reopen for the read side.
            let mut f = IoUringFile::open(&path).unwrap();
            let mut out = vec![0u8; 4096];
            f.read_all_at(0, &mut out).unwrap();
            assert_eq!(&out[1000..1016], &[7u8; 16]);
            assert_eq!(out[0], 0);
            assert_eq!(out[1016], 0);
        }
    }
}

// ---------------------------------------------------------------------------
// BlobStore integration
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "linux", feature = "io-uring"))]
pub mod store {
    //! [`BlobStore`](crate::store::BlobStore) integration: an
    //! io_uring-backed filesystem store with `LocalStore`-equivalent
    //! atomicity, plus roundtrip/parity tests.
    //!
    //! Tests exercise failure paths directly; unwrap/expect, slicing, and
    //! panicking asserts are acceptable here — violations surface as test
    //! failures, not production panics.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use std::path::PathBuf;

    use async_trait::async_trait;
    use bytes::Bytes;

    use crate::error::{BlobError, Result};
    use crate::store::BlobStore;
    use crate::types::{BlobId, BlobMetadata, ObjectKey};
    use crate::{local, types};

    pub use super::backend::{IoUringFile, DEFAULT_CHUNK_SIZE, DEFAULT_RING_SIZE};

    /// Filesystem-backed blob store using io_uring for the bulk data path.
    ///
    /// Same on-disk layout and atomicity contract as
    /// [`LocalStore`](crate::local::LocalStore): `put` writes a tempfile via
    /// io_uring, fsyncs (through `std`, see module docs), and renames into
    /// place. See the module docs for scope notes before using.
    #[derive(Debug)]
    pub struct IoUringStore {
        root: PathBuf,
        max_bytes: Option<u64>,
    }

    impl IoUringStore {
        /// Create a new store rooted at `root`. The directory is created if
        /// it does not exist.
        ///
        /// # Errors
        /// `Io` if the directory cannot be created or is not a directory.
        pub async fn new(root: PathBuf) -> Result<Self> {
            Self::with_limits(root, None).await
        }

        /// Create a new store with an optional maximum object size (same
        /// semantics as [`LocalStore::with_limits`](local::LocalStore::with_limits)).
        ///
        /// # Errors
        /// As [`new`](IoUringStore::new).
        pub async fn with_limits(root: PathBuf, max_bytes: Option<u64>) -> Result<Self> {
            tokio::fs::create_dir_all(&root).await?;
            let meta = tokio::fs::metadata(&root).await?;
            if !meta.is_dir() {
                return Err(BlobError::Other(format!(
                    "not a directory: {}",
                    root.display()
                )));
            }
            Ok(Self { root, max_bytes })
        }

        /// Create without touching the filesystem.
        #[must_use]
        pub fn new_unchecked(root: PathBuf) -> Self {
            Self {
                root,
                max_bytes: None,
            }
        }

        /// Resolve `key` under `root` (same traversal guard as `LocalStore`).
        fn resolve(&self, key: &ObjectKey) -> Result<PathBuf> {
            if key.as_str().contains("..") {
                return Err(BlobError::invalid_key("key contains '..'"));
            }
            Ok(self.root.join(key.as_str()))
        }
    }

    #[async_trait]
    impl BlobStore for IoUringStore {
        async fn put(&self, key: ObjectKey, data: Bytes) -> Result<BlobId> {
            if let Some(limit) = self.max_bytes {
                if data.len() as u64 > limit {
                    return Err(BlobError::StorageFull);
                }
            }

            let dest = self.resolve(&key)?;
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let parent = dest.parent().unwrap_or(&self.root).to_path_buf();

            let tmp = tempfile::Builder::new()
                .prefix(".blobkit-ring-")
                .tempfile_in(parent)
                .map_err(|e| BlobError::Other(format!("tempfile create: {e}")))?;

            let payload_len = data.len() as u64;

            // Dead stores, kept for parity with `LocalStore::put`.
            #[cfg(feature = "sha2")]
            let _ = crate::types::compute_sha256(&data);

            // Ring writes + fsync block; run them off the async runtime
            // (see module docs). The tempfile itself moves into the
            // blocking task and comes back for `persist`, so the inode
            // the rename publishes is the one io_uring filled.
            let tmp = tokio::task::spawn_blocking(move || -> std::io::Result<_> {
                let dup = tmp.as_file().try_clone()?;
                let mut f = IoUringFile::from_file(dup)?;
                f.write_all_at(0, &data)?;
                f.file().sync_all()?;
                Ok(tmp)
            })
            .await
            .map_err(|e| BlobError::Other(format!("blocking put: {e}")))?
            .map_err(|e| BlobError::Other(format!("io_uring write: {e}")))?;

            tmp.persist(&dest).map_err(BlobError::from)?;

            let _meta = BlobMetadata::new(key.clone(), payload_len)
                .with_content_type(types::guess_content_type(&key));

            Ok(BlobId::new())
        }

        async fn get(&self, key: &ObjectKey) -> Result<Bytes> {
            let path = self.resolve(key)?;
            // Ring reads block; run off the async runtime (see module docs).
            let read = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
                let mut f = IoUringFile::open(&path)?;
                let len = f.file().metadata()?.len() as usize;
                let mut buf = vec![0u8; len];
                f.read_all_at(0, &mut buf)?;
                Ok(buf)
            })
            .await
            .map_err(|e| BlobError::Other(format!("blocking get: {e}")))?;
            match read {
                Ok(v) => Ok(Bytes::from(v)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Err(BlobError::not_found(key.as_str()))
                }
                Err(e) => Err(BlobError::from(e)),
            }
        }

        async fn delete(&self, key: &ObjectKey) -> Result<()> {
            // unlink/stat are single syscalls; the ring adds nothing.
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
        async fn presigned_url(
            &self,
            key: &ObjectKey,
            expires: core::time::Duration,
        ) -> Result<url::Url> {
            local::LocalStore::new_unchecked(self.root.clone())
                .presigned_url(key, expires)
                .await
        }

        #[cfg(not(feature = "s3"))]
        async fn presigned_url(
            &self,
            key: &ObjectKey,
            expires: core::time::Duration,
        ) -> Result<String> {
            local::LocalStore::new_unchecked(self.root.clone())
                .presigned_url(key, expires)
                .await
        }
    }

    #[cfg(all(test, feature = "io-uring"))]
    mod tests {
        use super::*;
        use crate::local::LocalStore;

        #[tokio::test]
        async fn roundtrip_large_object() {
            let dir = tempfile::tempdir().unwrap();
            let store = IoUringStore::new(dir.path().to_path_buf()).await.unwrap();
            let payload: Vec<u8> = (0..3 * 1024 * 1024usize).map(|i| (i % 249) as u8).collect();
            let key = ObjectKey::new("big/object.bin").unwrap();
            store
                .put(key.clone(), Bytes::from(payload.clone()))
                .await
                .unwrap();
            let got = store.get(&key).await.unwrap();
            assert_eq!(got.len(), payload.len());
            assert_eq!(got, Bytes::from(payload));
        }

        #[tokio::test]
        async fn parity_with_local_store() {
            let payload: Vec<u8> = (0..2 * DEFAULT_CHUNK_SIZE + 777usize)
                .map(|i| (i % 253) as u8)
                .collect();
            let ring_dir = tempfile::tempdir().unwrap();
            let local_dir = tempfile::tempdir().unwrap();
            let ring = IoUringStore::new(ring_dir.path().to_path_buf())
                .await
                .unwrap();
            let local = LocalStore::new(local_dir.path().to_path_buf())
                .await
                .unwrap();

            let key = ObjectKey::new("parity/data.bin").unwrap();
            let ring_bytes = Bytes::from(payload.clone());
            let local_bytes = Bytes::from(payload);
            ring.put(key.clone(), ring_bytes).await.unwrap();
            local.put(key.clone(), local_bytes).await.unwrap();

            assert_eq!(
                ring.get(&key).await.unwrap(),
                local.get(&key).await.unwrap()
            );

            // Overwrite parity: both stores observe the same second write.
            let v2 = Bytes::from_static(b"second version");
            ring.put(key.clone(), v2.clone()).await.unwrap();
            local.put(key.clone(), v2.clone()).await.unwrap();
            assert_eq!(ring.get(&key).await.unwrap(), v2);
            assert_eq!(local.get(&key).await.unwrap(), v2);

            // Missing-key parity.
            let missing = ObjectKey::new("parity/absent.bin").unwrap();
            assert!(ring.get(&missing).await.unwrap_err().is_not_found());
            assert!(local.get(&missing).await.unwrap_err().is_not_found());
        }
    }
}
