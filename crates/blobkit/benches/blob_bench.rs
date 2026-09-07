// Benchmarks run on fixed, known-good inputs; unwrap failures abort the
// bench run visibly, which is the desired behavior here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use blobkit::memory::MemoryStore;
use blobkit::store::BlobStore;
use blobkit::types::ObjectKey;
use bytes::Bytes;
use criterion::{criterion_group, criterion_main, Criterion, Throughput};

fn bench_memory_put(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("memory_put");
    for size in [64usize, 1024, 16384] {
        let data = Bytes::from(vec![b'x'; size]);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(format!("put_{size}b"), &size, |b, _| {
            b.to_async(&rt).iter(|| async {
                let store = MemoryStore::new();
                let key = ObjectKey::new(format!("bench/{size}.bin")).unwrap();
                store.put(key, data.clone()).await.unwrap();
            });
        });
    }
    group.finish();
}

fn bench_memory_get(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("memory_get");
    for size in [64usize, 1024, 16384] {
        let data = Bytes::from(vec![b'x'; size]);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(format!("get_{size}b"), &size, |b, _| {
            // Pre-populate
            let store = MemoryStore::new();
            let key = ObjectKey::new(format!("bench/get_{size}.bin")).unwrap();
            rt.block_on(async {
                store.put(key.clone(), data.clone()).await.unwrap();
            });
            let key_clone = key.clone();
            b.to_async(&rt).iter(|| async {
                let _ = store.get(&key_clone).await.unwrap();
            });
        });
    }
    group.finish();
}

fn bench_local_put(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("local_put");
    for size in [64usize, 1024, 16384] {
        let data = Bytes::from(vec![b'x'; size]);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(format!("put_{size}b"), &size, |b, _| {
            b.to_async(&rt).iter(|| async {
                let dir = tempfile::tempdir().unwrap();
                let store = blobkit::local::LocalStore::new(dir.path().to_path_buf())
                    .await
                    .unwrap();
                let key = ObjectKey::new(format!("bench/{size}.bin")).unwrap();
                store.put(key, data.clone()).await.unwrap();
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_memory_put, bench_memory_get, bench_local_put);
criterion_main!(benches);
