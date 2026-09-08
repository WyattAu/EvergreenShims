use criterion::{Criterion, criterion_group, criterion_main};
use loop_retry::RetryConfig;
use std::time::Duration;

fn bench_retry_config_default(c: &mut Criterion) {
    c.bench_function("retry_config_default", |b| {
        b.iter(|| {
            let config = RetryConfig::default();
            std::hint::black_box(config);
        });
    });
}

fn bench_retry_config_no_retries(c: &mut Criterion) {
    c.bench_function("retry_config_no_retries", |b| {
        b.iter(|| {
            let config = RetryConfig::no_retries();
            std::hint::black_box(config);
        });
    });
}

fn bench_retry_config_aggressive(c: &mut Criterion) {
    c.bench_function("retry_config_aggressive", |b| {
        b.iter(|| {
            let config = RetryConfig::aggressive();
            std::hint::black_box(config);
        });
    });
}

fn bench_delay_for_attempt_no_jitter(c: &mut Criterion) {
    let config = RetryConfig {
        max_retries: 10,
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(5),
        backoff_multiplier: 2.0,
        jitter: false,
    };
    c.bench_function("delay_for_attempt_no_jitter", |b| {
        b.iter(|| {
            for attempt in 0..10 {
                let delay = config.delay_for_attempt(attempt);
                std::hint::black_box(delay);
            }
        });
    });
}

fn bench_delay_for_attempt_with_jitter(c: &mut Criterion) {
    let config = RetryConfig {
        max_retries: 10,
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(5),
        backoff_multiplier: 2.0,
        jitter: true,
    };
    c.bench_function("delay_for_attempt_with_jitter", |b| {
        b.iter(|| {
            for attempt in 0..10 {
                let delay = config.delay_for_attempt(attempt);
                std::hint::black_box(delay);
            }
        });
    });
}

fn bench_delay_for_high_attempt(c: &mut Criterion) {
    let config = RetryConfig {
        max_retries: 100,
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(200),
        backoff_multiplier: 2.0,
        jitter: false,
    };
    c.bench_function("delay_for_high_attempt_capped", |b| {
        b.iter(|| {
            let delay = config.delay_for_attempt(100);
            std::hint::black_box(delay);
        });
    });
}

fn bench_delay_for_low_attempt(c: &mut Criterion) {
    let config = RetryConfig::default();
    c.bench_function("delay_for_low_attempt", |b| {
        b.iter(|| {
            let delay = config.delay_for_attempt(0);
            std::hint::black_box(delay);
        });
    });
}

criterion_group!(
    benches,
    bench_retry_config_default,
    bench_retry_config_no_retries,
    bench_retry_config_aggressive,
    bench_delay_for_attempt_no_jitter,
    bench_delay_for_attempt_with_jitter,
    bench_delay_for_high_attempt,
    bench_delay_for_low_attempt,
);
criterion_main!(benches);
