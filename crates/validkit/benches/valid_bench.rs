// Benchmarks run on fixed, known-good inputs; unwrap failures abort the
// bench run visibly, which is the desired behavior here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use validkit::{
    BucketName, CronExpr, EmailAddr, FlagName, HttpsUrl, LocaleTag, ObjectKey, PhoneE164,
    TenantIdSlug,
};

fn bench_email(c: &mut Criterion) {
    c.bench_function("parse_email_valid", |b| {
        b.iter(|| EmailAddr::parse(black_box("user@example.com")).unwrap())
    });
    c.bench_function("parse_email_invalid", |b| {
        b.iter(|| EmailAddr::parse(black_box("not-an-email")))
    });
    c.bench_function("is_valid_email", |b| {
        b.iter(|| validkit::is_valid_email(black_box("user@example.com")))
    });
}

fn bench_url(c: &mut Criterion) {
    c.bench_function("parse_https_url_valid", |b| {
        b.iter(|| HttpsUrl::parse(black_box("https://example.com/path?q=1")).unwrap())
    });
    c.bench_function("parse_https_url_invalid", |b| {
        b.iter(|| HttpsUrl::parse(black_box("http://example.com")))
    });
}

fn bench_phone(c: &mut Criterion) {
    c.bench_function("parse_phone_valid", |b| {
        b.iter(|| PhoneE164::parse(black_box("+14155552671")).unwrap())
    });
    c.bench_function("parse_phone_invalid", |b| {
        b.iter(|| PhoneE164::parse(black_box("14155552671")))
    });
}

fn bench_cron(c: &mut Criterion) {
    c.bench_function("parse_cron_valid", |b| {
        b.iter(|| CronExpr::parse(black_box("*/5 * * * *")).unwrap())
    });
    c.bench_function("parse_cron_invalid", |b| {
        b.iter(|| CronExpr::parse(black_box("invalid cron")))
    });
}

fn bench_tenant(c: &mut Criterion) {
    c.bench_function("parse_tenant_valid", |b| {
        b.iter(|| TenantIdSlug::parse(black_box("my-tenant-123")).unwrap())
    });
    c.bench_function("parse_tenant_invalid", |b| {
        b.iter(|| TenantIdSlug::parse(black_box("AB")))
    });
}

fn bench_locale(c: &mut Criterion) {
    c.bench_function("parse_locale_valid", |b| {
        b.iter(|| LocaleTag::parse(black_box("en-US")).unwrap())
    });
    c.bench_function("parse_locale_invalid", |b| {
        b.iter(|| LocaleTag::parse(black_box("EN")))
    });
}

fn bench_flag(c: &mut Criterion) {
    c.bench_function("parse_flag_valid", |b| {
        b.iter(|| FlagName::parse(black_box("my_flag_123")).unwrap())
    });
    c.bench_function("parse_flag_invalid", |b| {
        b.iter(|| FlagName::parse(black_box("My-Flag")))
    });
}

fn bench_bucket(c: &mut Criterion) {
    c.bench_function("parse_bucket_valid", |b| {
        b.iter(|| BucketName::parse(black_box("my-bucket-123")).unwrap())
    });
    c.bench_function("parse_bucket_invalid", |b| {
        b.iter(|| BucketName::parse(black_box("192.168.1.1")))
    });
}

fn bench_object_key(c: &mut Criterion) {
    c.bench_function("parse_object_key_valid", |b| {
        b.iter(|| ObjectKey::parse(black_box("foo/bar/baz.txt")).unwrap())
    });
    c.bench_function("parse_object_key_invalid", |b| {
        b.iter(|| ObjectKey::parse(black_box("/foo/bar")))
    });
}

criterion_group!(
    benches,
    bench_email,
    bench_url,
    bench_phone,
    bench_cron,
    bench_tenant,
    bench_locale,
    bench_flag,
    bench_bucket,
    bench_object_key
);
criterion_main!(benches);
