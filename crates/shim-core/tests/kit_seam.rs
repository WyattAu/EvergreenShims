//! Estate-crate seam tests (validkit / blobkit / loop-retry).
//!
//! The workspace previously vendored these crates; it now consumes the
//! published crates.io versions. These tests pin the seam contracts the
//! shims rely on: HTTPS/slug/cron validation, local blob round-trip with a
//! max_bytes guard, and the retry policy's exponential cap.

use std::time::Duration;

use blobkit::local::LocalStore;
use blobkit::BlobStore;
use blobkit::{BucketName, ObjectKey};
use loop_retry::RetryConfig;
use validkit::{CronExpr, HttpsUrl, TenantIdSlug};

// --- validkit ---

#[test]
fn https_url_accepts_https_and_rejects_plain_http() {
    HttpsUrl::new("https://registry.example.com".to_string()).expect("https URL must validate");
    assert!(
        HttpsUrl::new("http://registry.example.com".to_string()).is_err(),
        "plain http must be rejected"
    );
    assert!(
        HttpsUrl::new("".to_string()).is_err(),
        "empty must be rejected"
    );
}

#[test]
fn tenant_slug_rejects_path_traversal_and_bad_chars() {
    TenantIdSlug::try_from("acme-corp").expect("simple slug must validate");
    assert!(
        TenantIdSlug::try_from("../etc").is_err(),
        "traversal must be rejected"
    );
    assert!(
        TenantIdSlug::try_from("a b").is_err(),
        "spaces must be rejected"
    );
    assert!(
        TenantIdSlug::try_from("").is_err(),
        "empty must be rejected"
    );
}

#[test]
fn cron_expr_accepts_known_forms_and_rejects_garbage() {
    CronExpr::try_from("0 3 * * *").expect("daily cron must parse");
    assert!(CronExpr::try_from("not a cron").is_err());
    assert!(CronExpr::try_from("").is_err());
}

// --- blobkit ---

#[tokio::test]
async fn local_store_round_trips_object_and_respects_max_bytes() {
    let root = tempfile::tempdir().unwrap();

    let store = LocalStore::with_limits(root.path().to_path_buf(), Some(64))
        .await
        .expect("store must create root dir");

    let key = ObjectKey::try_from("shim/seam/test-object.bin").unwrap();
    let payload = bytes::Bytes::from_static(b"archival payload");

    store
        .put(key.clone(), payload.clone())
        .await
        .expect("put must succeed");
    let fetched = store.get(&key).await.expect("get must succeed");
    assert_eq!(fetched, payload, "round-trip must preserve bytes");

    let oversized = bytes::Bytes::from(vec![0u8; 128]);
    let err = store
        .put(
            ObjectKey::try_from("shim/seam/oversized.bin").unwrap(),
            oversized,
        )
        .await;
    assert!(err.is_err(), "payload over max_bytes must be rejected");
}

#[test]
fn blobkit_typed_newtypes_reject_bad_names() {
    assert!(BucketName::new("valid-bucket").is_ok());
    assert!(BucketName::new("").is_err());
    assert!(ObjectKey::try_from("valid/key.txt").is_ok());
}

// --- loop-retry ---

#[test]
fn retry_delay_grows_exponentially_and_stays_capped() {
    let cfg = RetryConfig {
        max_retries: 8,
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(5),
        backoff_multiplier: 2.0,
        jitter: false,
    };

    let d0 = cfg.delay_for_attempt(0);
    let d2 = cfg.delay_for_attempt(2);
    assert_eq!(
        d0,
        Duration::from_millis(100),
        "first delay is the initial delay"
    );
    assert_eq!(
        d2,
        Duration::from_millis(400),
        "2^2 growth after two attempts"
    );

    // Far-out attempts must clamp to max_delay, not overflow.
    for attempt in [10u32, 20, 63] {
        assert_eq!(
            cfg.delay_for_attempt(attempt),
            Duration::from_secs(5),
            "delay must cap at max_delay"
        );
    }
}

#[test]
fn retry_jitter_never_reduces_delay_below_base() {
    let cfg = RetryConfig {
        max_retries: 4,
        initial_delay: Duration::from_millis(1_000),
        max_delay: Duration::from_secs(60),
        backoff_multiplier: 2.0,
        jitter: true,
    };
    for _ in 0..50 {
        let d = cfg.delay_for_attempt(1);
        // Base for attempt 1 is 2×initial (multiplier first), then up to
        // 10% jitter on top: [2000ms, 2200ms].
        assert!(
            d >= Duration::from_millis(2_000) && d <= Duration::from_millis(2_200),
            "jitter only adds up to 10%, got {d:?}"
        );
    }
}
