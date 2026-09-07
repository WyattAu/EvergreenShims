// Benchmarks run on fixed, known-good inputs; unwrap failures abort the
// bench run visibly, which is the desired behavior here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

//! Sequential 1 MiB write/read: `tokio::fs` vs the io_uring path.
//!
//! Notes for honest reading of the numbers:
//! - The io_uring timings include per-iteration ring setup (a handful of
//!   syscalls) — the backend intentionally scopes itself to one ring per
//!   file/op in this minimal version, and `write_all_at`/`read_all_at`
//!   block their calling thread (see `io_uring_backend` module docs).
//! - Neither side fsyncs inside the timed section, so this measures bulk
//!   data transfer, not durability overhead.
//! - 1 MiB at the default 128 KiB chunk = 8 SQEs, comfortably inside the
//!   default ring depth of 64 (one submit batch).

use blobkit::store::BlobStore;
use blobkit::{IoUringFile, IoUringStore};
use bytes::Bytes;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use tokio::io::AsyncReadExt;

const SIZE: usize = 1024 * 1024;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn bench_write_1m(c: &mut Criterion) {
    let data = vec![b'x'; SIZE];
    let dir = tempfile::tempdir().unwrap();
    let tokio_path = dir.path().join("tokio.bin");
    let ring_path = dir.path().join("ring.bin");

    let mut group = c.benchmark_group("sequential_1m_write");
    group.throughput(Throughput::Bytes(SIZE as u64));

    group.bench_function("tokio_fs", |b| {
        b.to_async(&rt()).iter(|| async {
            tokio::fs::write(&tokio_path, &data).await.unwrap();
        });
    });

    group.bench_function("io_uring", |b| {
        b.to_async(&rt()).iter(|| async {
            let mut f = IoUringFile::create(&ring_path).unwrap();
            f.write_all_at(0, &data).unwrap();
        });
    });

    group.finish();
}

fn bench_read_1m(c: &mut Criterion) {
    let data = vec![b'x'; SIZE];
    let dir = tempfile::tempdir().unwrap();
    let tokio_path = dir.path().join("tokio.bin");
    let ring_path = dir.path().join("ring.bin");
    std::fs::write(&tokio_path, &data).unwrap();
    std::fs::write(&ring_path, &data).unwrap();

    let mut group = c.benchmark_group("sequential_1m_read");
    group.throughput(Throughput::Bytes(SIZE as u64));

    // Buffer allocation is setup (untimed); the read itself is timed.
    group.bench_function("tokio_fs", |b| {
        b.to_async(&rt()).iter_batched(
            || vec![0u8; SIZE],
            |mut buf| {
                let path = tokio_path.clone();
                async move {
                    let mut f = tokio::fs::File::open(&path).await.unwrap();
                    f.read_exact(&mut buf).await.unwrap();
                    buf
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("io_uring", |b| {
        b.to_async(&rt()).iter_batched(
            || vec![0u8; SIZE],
            |mut buf| {
                let path = ring_path.clone();
                async move {
                    let mut f = IoUringFile::open(&path).unwrap();
                    f.read_all_at(0, &mut buf).unwrap();
                    buf
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_store_put_get_1m(c: &mut Criterion) {
    let data = Bytes::from(vec![b'x'; SIZE]);

    let mut group = c.benchmark_group("sequential_1m_store");
    group.throughput(Throughput::Bytes(SIZE as u64));

    group.bench_function("io_uring_store_put", |b| {
        b.to_async(&rt()).iter(|| async {
            let dir = tempfile::tempdir().unwrap();
            let store = IoUringStore::new(dir.path().to_path_buf()).await.unwrap();
            let key = blobkit::types::ObjectKey::new("bench/1m.bin").unwrap();
            store.put(key, data.clone()).await.unwrap();
        });
    });

    group.bench_function("io_uring_store_get", |b| {
        let dir = tempfile::tempdir().unwrap();
        let store = rt().block_on(async {
            let s = IoUringStore::new(dir.path().to_path_buf()).await.unwrap();
            let key = blobkit::types::ObjectKey::new("bench/1m.bin").unwrap();
            s.put(key, data.clone()).await.unwrap();
            s
        });
        let key = blobkit::types::ObjectKey::new("bench/1m.bin").unwrap();
        b.to_async(&rt()).iter(|| async {
            let _ = store.get(&key).await.unwrap();
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_write_1m,
    bench_read_1m,
    bench_store_put_get_1m
);
criterion_main!(benches);
