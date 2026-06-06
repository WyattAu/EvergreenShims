use backup_shim::BackupShim;
use cache_shim::CacheShim;
use criterion::{criterion_group, criterion_main, Criterion};
use encryption_shim::EncryptionShim;

fn bench_backup_checksum(c: &mut Criterion) {
    let data = vec![0u8; 1024 * 1024]; // 1MB
    c.bench_function("backup_checksum_1mb", |b| {
        b.iter(|| BackupShim::compute_checksum(&data));
    });
}

fn bench_cache_set_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_operations");
    let shim = CacheShim::new();

    group.bench_function("set_1000_keys", |b| {
        b.iter(|| {
            for i in 0..1000 {
                shim.set(&format!("key_{}", i), b"value_data");
            }
        });
    });

    group.bench_function("get_1000_keys", |b| {
        for i in 0..1000 {
            shim.set(&format!("key_{}", i), b"value_data");
        }
        b.iter(|| {
            for i in 0..1000 {
                shim.get(&format!("key_{}", i));
            }
        });
    });

    group.finish();
}

fn bench_encryption(c: &mut Criterion) {
    let mut shim = EncryptionShim::new();
    let data = vec![0u8; 4096]; // 4KB

    let mut group = c.benchmark_group("encryption_throughput");
    group.bench_function("aes_gcm_encrypt_4kb", |b| {
        b.iter(|| shim.encrypt(&data, None).unwrap());
    });

    let encrypted = shim.encrypt(&data, None).unwrap();
    group.bench_function("aes_gcm_decrypt_4kb", |b| {
        b.iter(|| shim.decrypt(&encrypted).unwrap());
    });

    group.finish();
}

fn bench_migration_checksum(c: &mut Criterion) {
    let sql = "CREATE TABLE users (id SERIAL PRIMARY KEY, name VARCHAR(255) NOT NULL, email VARCHAR(255) UNIQUE NOT NULL, created_at TIMESTAMP DEFAULT NOW());";
    c.bench_function("migration_checksum", |b| {
        b.iter(|| migration_shim::MigrationShim::compute_checksum(sql));
    });
}

criterion_group!(
    benches,
    bench_backup_checksum,
    bench_cache_set_get,
    bench_encryption,
    bench_migration_checksum,
);
criterion_main!(benches);
