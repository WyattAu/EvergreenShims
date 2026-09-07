#![cfg_attr(not(feature = "std"), no_std)]
// `forbid(unsafe_code)` crate-wide, with one exception: the `io-uring`
// backend shares ring memory with the kernel and needs `unsafe` blocks
// (each annotated with its invariant — see `io_uring_backend`). `forbid`
// cannot be overridden by an inner `allow` (E0453), so it is only enforced
// when that backend is not compiled; `deny` backs it up otherwise and only
// the backend module opts out.
#![cfg_attr(
    not(all(feature = "io-uring", target_os = "linux")),
    forbid(unsafe_code)
)]
#![deny(unsafe_code)]
#![deny(missing_docs)]

//! # blobkit
//!
//! Unified blob storage for Rust — trait + Memory + Local + S3.
//!
//! Replaces `S3Client::from_conf` duplication across
//! `archival-shim`, `backup-shim`, `kestrel-storage`, and `ferro` with a
//! single [`store::BlobStore`] trait.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use blobkit::memory::MemoryStore;
//! use blobkit::store::BlobStore;
//! use blobkit::types::ObjectKey;
//! use bytes::Bytes;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), blobkit::error::BlobError> {
//! let store = MemoryStore::new();
//! let key = ObjectKey::new("hello.txt")?;
//! store.put(key.clone(), Bytes::from("hello world")).await?;
//! let data = store.get(&key).await?;
//! assert_eq!(data, Bytes::from("hello world"));
//! # Ok(())
//! # }
//! ```
//!
//! ## Backends
//!
//! | Backend | Feature | Durable | Distribution | Notes |
//! |---------|---------|---------|--------------|-------|
//! | [`memory::MemoryStore`] | `memory` | No | Single-process | For tests |
//! | [`local::LocalStore`] | `std` | Yes | Single-node | Atomic writes |
//! | [`io_uring_backend::store::IoUringStore`] | `io-uring` | Yes | Single-node (Linux) | io_uring bulk path, atomic writes |
//! | [`s3::S3Store`] | `s3` | Yes | Distributed | aws-sdk-s3, path-style + custom endpoints |
//!
//! ## Features
//!
//! - `std` (default): enable `std::io` errors and filesystem backend.
//! - `memory`: enable in-memory backend via `dashmap`.
//! - `s3`: enable the real S3 backend (`aws-sdk-s3` + `aws-config`) with
//!   pre-signed URLs, path-style addressing, custom endpoints, and static
//!   or chain-based credentials.
//! - `io-uring` (Linux only): enable [`io_uring_backend`] — a `LocalStore`
//!   variant whose bulk `pread`/`pwrite` path uses raw io_uring SQEs. The
//!   blocking submit-wait sections run inside `spawn_blocking`; the raw
//!   blocking primitives are documented there.
//! - `typed-id`: enable `BlobId` backed by `uuid::Uuid`.
//! - `sha2`: compute `sha256` in metadata.
//! - `chrono`: attach `created_at` timestamps.
//! - `serde`: derive `Serialize`/`Deserialize` on key types.
//! - `tracing`: emit `trace`/`debug` spans for store operations.

extern crate alloc;

pub mod error;
pub mod local;
pub mod memory;
pub mod s3;
pub mod store;
pub mod types;

#[cfg(all(feature = "io-uring", target_os = "linux"))]
#[allow(unsafe_code)]
pub mod io_uring_backend;

// Re-exports for ergonomic imports.
pub use error::{BlobError, Result};
pub use store::BlobStore;
pub use types::{BlobId, BlobMetadata, BucketName, ObjectKey};

#[cfg(feature = "s3")]
pub use s3::{S3Config, S3Store};

#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub use io_uring_backend::store::{IoUringFile, IoUringStore};
