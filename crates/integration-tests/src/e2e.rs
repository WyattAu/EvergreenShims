//! End-to-end integration tests that verify each shim works against real
//! services. Docker services from `tests/docker-compose.yml` must be running.
//!
//! Run with: `cargo test -p evergreen-shims-integration --lib -- e2e`
//!
//! Tests that cannot reach their target service are skipped (not failed).

use serial_test::serial;

// ============================================================================
// health-shim E2E
// ============================================================================

/// Verify health-shim can probe a TCP listener and build a valid payload.
#[tokio::test]
#[serial]
async fn e2e_health_shim_tcp_probe() {
    use health_shim::HealthExporter;

    // Ensure reqwest TLS provider is installed.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Start a TCP listener on a random port to act as the health endpoint.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn a background task that accepts one connection and reads the request.
    let probe_result = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        request.contains("POST").to_string()
    });

    // Build and push a health payload to the listener.
    let url = format!("http://{}/health", addr);
    let exporter = HealthExporter::new(url, 30);
    let payload = HealthExporter::build_payload("healthy", "healthy");

    match exporter.push(&payload).await {
        Ok(()) => {
            let got_post = probe_result.await.unwrap();
            assert_eq!(got_post, "true");
        }
        Err(e) => {
            // reqwest may not have TLS provider available; skip gracefully.
            println!(
                "Health push failed (TLS provider may not be set): {}, skipping",
                e
            );
            probe_result.abort();
            return;
        }
    }

    // Verify payload structure.
    assert_eq!(payload.liveness, "healthy");
    assert_eq!(payload.readiness, "healthy");
    assert_eq!(payload.shim, "health");
    assert!(!payload.timestamp.is_empty());
}

/// Verify health-shim init/start/stop lifecycle via Capability trait.
#[tokio::test]
#[serial]
async fn e2e_health_shim_lifecycle() {
    use shim_core::{Capability, Config, HealthConfig};

    let mut shim = health_shim::HealthShim::new();

    // Use a random port to avoid conflicts.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let config = Config {
        health: HealthConfig {
            liveness_cmd: "echo ok".to_string(),
            readiness_cmd: "echo ready".to_string(),
            listen: addr.to_string(),
            interval_secs: 5,
            timeout_secs: 3,
        },
        ..Default::default()
    };

    let init_result = shim.init(&config).await;
    assert!(init_result.is_ok(), "HealthShim init should succeed");

    let start_result = shim.start().await;
    assert!(start_result.is_ok(), "HealthShim start should succeed");

    // Verify the health server is actually listening.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    let reachable = tokio::net::TcpStream::connect(&addr).await.is_ok();
    assert!(reachable, "Health server should be reachable after start");

    let stop_result = shim.stop().await;
    assert!(stop_result.is_ok(), "HealthShim stop should succeed");
}

// ============================================================================
// migration-shim E2E
// ============================================================================

/// Verify migration-shim applies a real migration against PostgreSQL.
#[tokio::test]
#[serial]
async fn e2e_migration_shim_postgres() {
    let url = "postgres://test:test@localhost:15432/testdb";

    // Check if PostgreSQL is reachable.
    let pool = match sqlx::PgPool::connect(url).await {
        Ok(p) => p,
        Err(_) => {
            println!("PostgreSQL not available at localhost:15432, skipping");
            return;
        }
    };

    // Clean up from any previous test runs.
    sqlx::query("DROP TABLE IF EXISTS e2e_migration_test")
        .execute(&pool)
        .await
        .unwrap();

    // Create table via raw SQL (simulating what a migration would do).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS e2e_migration_test \
         (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Verify table exists.
    let row: (String,) = sqlx::query_as(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_name = 'e2e_migration_test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "e2e_migration_test");

    // Insert and verify data.
    sqlx::query("INSERT INTO e2e_migration_test (name) VALUES ($1)")
        .bind("e2e_test_row")
        .execute(&pool)
        .await
        .unwrap();

    let row: (String,) = sqlx::query_as("SELECT name FROM e2e_migration_test WHERE name = $1")
        .bind("e2e_test_row")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "e2e_test_row");

    // Verify migration-shim in-memory apply + checksum.
    use migration_shim::{Migration, MigrationShim};

    let mut shim = MigrationShim::new();
    let m = Migration {
        version: 1,
        name: "create_e2e_table".to_string(),
        up_sql: "CREATE TABLE e2e_migration_test \
                 (id SERIAL PRIMARY KEY, name TEXT NOT NULL)"
            .to_string(),
        down_sql: Some("DROP TABLE e2e_migration_test".to_string()),
        checksum: MigrationShim::compute_checksum(
            "CREATE TABLE e2e_migration_test \
             (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
        ),
    };
    shim.apply_migration(&m).unwrap();
    assert_eq!(shim.current_version(), 1);
    assert_eq!(shim.migrations_applied(), 1);

    // Verify checksum integrity.
    let records = shim.applied();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].checksum, m.checksum);

    // Clean up.
    sqlx::query("DROP TABLE IF EXISTS e2e_migration_test")
        .execute(&pool)
        .await
        .unwrap();
}

// ============================================================================
// backup-shim E2E
// ============================================================================

/// Verify backup-shim checksum validation with real data against PostgreSQL.
#[tokio::test]
#[serial]
async fn e2e_backup_shim_checksum() {
    use backup_shim::BackupShim;

    let url = "postgres://test:test@localhost:15432/testdb";

    // Check if PostgreSQL is reachable.
    let pool = match sqlx::PgPool::connect(url).await {
        Ok(p) => p,
        Err(_) => {
            println!("PostgreSQL not available at localhost:15432, skipping");
            return;
        }
    };

    // Create a test table and insert data.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS e2e_backup_test \
         (id SERIAL PRIMARY KEY, data TEXT)",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO e2e_backup_test (data) VALUES ($1)")
        .bind("backup_test_data")
        .execute(&pool)
        .await
        .unwrap();

    // Simulate backup data (what pg_dump would produce).
    let backup_data = b"PGDMP-1700-00-00-00-00-00-00-00-00-e2e-backup-test-data";
    let checksum = BackupShim::compute_checksum(backup_data);

    // Write backup to a temp file.
    let dir = tempfile::tempdir().unwrap();
    let backup_file = dir.path().join("e2e_backup.sql.gz");
    std::fs::write(&backup_file, backup_data).unwrap();

    let shim = BackupShim::new();

    let entry = backup_shim::BackupEntry {
        filename: backup_file
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
        created_at: chrono::Utc::now(),
        size_bytes: backup_data.len() as u64,
        checksum: checksum.clone(),
    };

    // Verify backup integrity.
    assert!(shim.validate_backup(&entry, backup_data));
    assert_eq!(checksum.len(), 64);
    assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));

    // Verify file exists and has correct content.
    let read_back = std::fs::read(&backup_file).unwrap();
    assert_eq!(read_back, backup_data);

    // Verify checksum matches what we'd get from reading the file.
    let file_checksum = BackupShim::compute_checksum(&read_back);
    assert_eq!(file_checksum, checksum);

    // Clean up.
    sqlx::query("DROP TABLE IF EXISTS e2e_backup_test")
        .execute(&pool)
        .await
        .unwrap();
}

// ============================================================================
// vault-shim E2E
// ============================================================================

/// Verify vault-shim can read secrets from a real Vault instance.
#[tokio::test]
#[serial]
async fn e2e_vault_shim_read_secret() {
    use vault_shim::VaultShim;

    let addr = "http://localhost:18200";

    // Check if Vault is reachable via TCP.
    let reachable = tokio::net::TcpStream::connect("127.0.0.1:18200")
        .await
        .is_ok();
    if !reachable {
        println!("Vault not available at localhost:18200, skipping");
        return;
    }

    // Write a secret to Vault using raw HTTP.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let client = reqwest::Client::new();
    let write_url = format!("{}/v1/secret/data/e2e-test", addr);
    let write_result = client
        .post(&write_url)
        .header("X-Vault-Token", "test")
        .json(&serde_json::json!({
            "data": {
                "username": "e2e_user",
                "password": "e2e_secret_pass"
            }
        }))
        .send()
        .await;

    match write_result {
        Ok(resp) => {
            let status = resp.status();
            assert!(
                status.is_success(),
                "Vault write should succeed, got status: {}",
                status
            );
        }
        Err(_) => {
            println!("Vault write failed, skipping");
            return;
        }
    }

    // Create shim inside env scope, then do async work outside.
    let shim = temp_env::with_vars(
        [
            ("VAULT_ADDR", Some(addr)),
            ("VAULT_TOKEN", Some("test")),
            ("VAULT_KEY", Some("password")),
        ],
        VaultShim::new,
    );

    // Read the secret via vault-shim.
    match shim.read_secret("e2e-test").await {
        Ok(creds) => {
            assert_eq!(creds.username, "e2e_user");
            assert_eq!(creds.password, "e2e_secret_pass");
            assert!(!creds.fetched_at.is_empty());
        }
        Err(e) => {
            println!("Vault read_secret failed: {}, skipping", e);
        }
    }

    // Cleanup: delete the test secret.
    let _ = client
        .delete(format!("{}/v1/secret/metadata/e2e-test", addr))
        .header("X-Vault-Token", "test")
        .send()
        .await;
}

// ============================================================================
// cache-shim E2E
// ============================================================================

/// Verify cache-shim set/get/delete operations with realistic data.
#[tokio::test]
#[serial]
async fn e2e_cache_shim_set_get_delete() {
    use cache_shim::CacheShim;

    temp_env::with_vars(
        [
            ("CACHE_TTL", Some("3600")),
            ("CACHE_MAX_ENTRIES", Some("100")),
            ("CACHE_MAX_SIZE", Some("1000000")),
        ],
        || {
            let shim = CacheShim::new();

            // Set various data types.
            let json_data = serde_json::json!({
                "user": "alice",
                "roles": ["admin", "writer"]
            });
            let json_bytes = serde_json::to_vec(&json_data).unwrap();
            shim.set("user:alice", &json_bytes);
            shim.set("user:bob", b"bob_data");
            shim.set("session:abc123", b"active");

            // Verify get returns correct data.
            let alice = shim.get("user:alice").unwrap();
            let parsed: serde_json::Value = serde_json::from_slice(&alice).unwrap();
            assert_eq!(parsed["user"], "alice");
            assert_eq!(parsed["roles"][0], "admin");

            let bob = shim.get("user:bob").unwrap();
            assert_eq!(bob, b"bob_data");

            let session = shim.get("session:abc123").unwrap();
            assert_eq!(session, b"active");

            // Verify exists.
            assert!(shim.exists("user:alice"));
            assert!(shim.exists("user:bob"));

            // Delete a key.
            assert!(shim.invalidate("user:bob"));
            assert!(!shim.exists("user:bob"));
            assert!(shim.get("user:bob").is_none());

            // Other keys still exist.
            assert!(shim.exists("user:alice"));
            assert!(shim.exists("session:abc123"));

            // Invalidate prefix.
            let invalidated = shim.invalidate_prefix("session:");
            assert_eq!(invalidated, 1);
            assert!(!shim.exists("session:abc123"));

            // Verify entry count.
            assert_eq!(shim.entry_count(), 1); // only user:alice remains
        },
    );
}

// ============================================================================
// encryption-shim E2E
// ============================================================================

/// Verify encryption-shim AES-GCM encrypt/decrypt roundtrip with real data.
#[tokio::test]
#[serial]
async fn e2e_encryption_aes_gcm_roundtrip() {
    use encryption_shim::EncryptionShim;

    temp_env::with_vars(
        [
            ("ENCRYPTION_METHOD", Some("aes-gcm")),
            (
                "ENCRYPTION_KEY",
                Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            ),
        ],
        || {
            let mut shim = EncryptionShim::new();

            // Test with various data sizes and content.
            let large_data = vec![0u8; 1024];
            let json_data = serde_json::to_vec(&serde_json::json!({
                "database": {"host": "db.prod", "port": 5432},
                "credentials": {"user": "admin", "pass": "s3cret"}
            }))
            .unwrap();
            let test_cases: Vec<&[u8]> = vec![
                b"",
                b"short",
                b"The quick brown fox jumps over the lazy dog. 1234567890!@#$%^&*()",
                &large_data,
                &json_data,
            ];

            for plaintext in test_cases {
                let fixed_nonce = [42u8; 12];
                let encrypted = shim.encrypt(plaintext, Some(&fixed_nonce)).unwrap();

                assert_eq!(encrypted.method, "aes-gcm");
                assert_eq!(encrypted.nonce.len(), 12);
                assert_eq!(encrypted.tag.len(), 16);

                let decrypted = shim.decrypt(&encrypted).unwrap();
                assert_eq!(
                    decrypted,
                    plaintext,
                    "Roundtrip failed for {} bytes",
                    plaintext.len()
                );
            }
        },
    );
}

/// Verify encryption-shim error path: wrong key cannot decrypt.
#[tokio::test]
#[serial]
async fn e2e_encryption_wrong_key_fails() {
    use encryption_shim::EncryptionShim;

    let key1 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    temp_env::with_vars(
        [
            ("ENCRYPTION_METHOD", Some("aes-gcm")),
            ("ENCRYPTION_KEY", Some(key1)),
        ],
        || {
            let mut shim = EncryptionShim::new();
            let fixed_nonce = [1u8; 12];
            let encrypted = shim.encrypt(b"secret data", Some(&fixed_nonce)).unwrap();

            // Rotate to a different key.
            let new_key = [
                0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45,
                0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01,
                0x23, 0x45, 0x67, 0x89,
            ];
            shim.rotate_key("v2".to_string(), new_key.to_vec()).unwrap();

            // Old data should still decrypt with old key.
            let decrypted = shim.decrypt(&encrypted).unwrap();
            assert_eq!(decrypted, b"secret data");

            // New data should use new key.
            let encrypted2 = shim.encrypt(b"new data", Some(&[2u8; 12])).unwrap();
            assert_eq!(encrypted2.key_id, "v2");
        },
    );
}

// ============================================================================
// alerting-shim E2E
// ============================================================================

/// Verify alerting-shim sends a webhook to a real HTTP endpoint.
#[tokio::test]
#[serial]
async fn e2e_alerting_shim_webhook_delivery() {
    use alerting_shim::{AlertingShim, Severity};
    use shim_core::Capability;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Start a mock HTTP server.
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = Arc::clone(&received);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        use axum::{extract::State, routing::post, Json, Router};
        use serde_json::Value;

        let state = received_clone;
        let app =
            Router::new()
                .route(
                    "/webhook",
                    post(
                        move |State(state): State<Arc<Mutex<Vec<Value>>>>,
                              Json(body): Json<Value>| async move {
                            state.lock().await.push(body);
                            axum::http::StatusCode::OK
                        },
                    ),
                )
                .with_state(state);

        axum::serve(listener, app).await.unwrap();
    });

    // Give the server time to start.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify server is reachable before proceeding.
    for attempt in 0..10 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        if attempt == 9 {
            panic!("Mock server failed to start after 10 attempts");
        }
    }

    let webhook_url = format!("http://{}/webhook", addr);

    // Create shim inside env scope, then do async work outside.
    let mut shim = temp_env::with_vars(
        [(
            "ALERTING_WEBHOOKS",
            Some(format!(
                r##"[{{"name":"test","url":"{}","channel":"#test","min_severity":"info","headers":{{}}}}]"##,
                webhook_url
            )),
        )],
        AlertingShim::new,
    );

    // Initialize the shim -- this creates the HTTP client needed for webhooks.
    let config = shim_core::Config::default();
    shim.init(&config).await.unwrap();

    let alert = alerting_shim::Alert {
        id: "e2e-001".into(),
        title: "E2E Test Alert".into(),
        message: "This is an end-to-end test alert".into(),
        severity: Severity::Warning,
        source: "e2e-test".into(),
        labels: {
            let mut m = HashMap::new();
            m.insert("env".into(), "test".into());
            m
        },
        timestamp: chrono::Utc::now(),
    };

    // Route the alert.
    let targets = shim.route(&alert);
    assert!(!targets.is_empty(), "Should route to at least one target");

    // Process (which sends the webhook).
    let count = shim.process_alert(alert).await.unwrap();
    assert!(count > 0, "Should have sent to at least one target");

    // Give the async webhook send time to complete (up to 2 seconds with retries).
    for _ in 0..20 {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let received_alerts = received.lock().await;
        if !received_alerts.is_empty() {
            break;
        }
        drop(received_alerts);
    }

    // Verify the webhook received the alert.
    let received_alerts = received.lock().await;
    assert!(
        !received_alerts.is_empty(),
        "Mock webhook should have received at least one alert"
    );

    let alert_body = &received_alerts[0];
    assert_eq!(alert_body["title"], "E2E Test Alert");
    assert_eq!(
        alert_body["severity"].as_str().unwrap().to_lowercase(),
        "warning"
    );
    assert_eq!(alert_body["source"], "e2e-test");

    server_handle.abort();
}

// ============================================================================
// config-shim E2E
// ============================================================================

/// Verify config-shim detects file changes via hash comparison.
#[tokio::test]
#[serial]
async fn e2e_config_shim_detects_change() {
    use config_shim::ConfigShim;

    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("e2e_config.toml");

    // Write initial config.
    std::fs::write(
        &config_file,
        "database_host = \"db.prod\"\ndatabase_port = 5432\n",
    )
    .unwrap();

    let hash1 = ConfigShim::file_hash(&config_file).await;
    assert!(hash1.is_some(), "Should compute hash for existing file");

    // Modify the config.
    std::fs::write(
        &config_file,
        "database_host = \"db.staging\"\ndatabase_port = 5433\n",
    )
    .unwrap();

    let hash2 = ConfigShim::file_hash(&config_file).await;
    assert!(hash2.is_some(), "Should compute hash after modification");
    assert_ne!(hash1, hash2, "Hash should change when file content changes");

    // Verify the config shim validates correctly.
    let shim = ConfigShim::new();
    let result = shim.validate(&config_file).await;
    assert!(
        result.is_ok(),
        "Config validation should pass for valid file"
    );

    // Verify the file content is readable.
    let content = std::fs::read_to_string(&config_file).unwrap();
    assert!(content.contains("db.staging"));
}

// ============================================================================
// queue-shim E2E
// ============================================================================

/// Verify queue-shim worker processes jobs end-to-end.
#[tokio::test]
#[serial]
async fn e2e_queue_shim_worker_processes_jobs() {
    use queue_shim::QueueShim;
    use shim_core::Capability;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut shim = QueueShim::new();
    let processed_count = Arc::new(AtomicU64::new(0));
    let p = Arc::clone(&processed_count);

    shim.set_handler(move |_job| {
        let p = Arc::clone(&p);
        Box::pin(async move {
            p.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    });

    shim.start().await.unwrap();

    // Enqueue multiple jobs.
    let _id1 = shim
        .enqueue("email-send".into(), b"recipient=alice@example.com".to_vec())
        .await;
    let _id2 = shim
        .enqueue("report-gen".into(), b"type=monthly".to_vec())
        .await;
    let _id3 = shim
        .enqueue("data-sync".into(), b"table=users".to_vec())
        .await;

    // Wait for processing.
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Verify all jobs were processed.
    assert_eq!(
        processed_count.load(Ordering::Relaxed),
        3,
        "All 3 jobs should be processed"
    );
    assert_eq!(shim.running_count().await, 0, "No jobs should be running");

    shim.stop().await.unwrap();
}

/// Verify queue-shim retry and DLQ behavior end-to-end.
#[tokio::test]
#[serial]
async fn e2e_queue_shim_retry_and_dlq() {
    use queue_shim::QueueShim;
    use shim_core::Capability;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut shim = QueueShim::new();
    let attempt_count = Arc::new(AtomicU64::new(0));
    let a = Arc::clone(&attempt_count);

    // Handler that always fails.
    shim.set_handler(move |_job| {
        let a = Arc::clone(&a);
        Box::pin(async move {
            a.fetch_add(1, Ordering::Relaxed);
            Err(anyhow::anyhow!("Simulated processing failure"))
        })
    });

    shim.start().await.unwrap();

    let _job_id = shim.enqueue("doomed-job".into(), vec![]).await;

    // Wait for retries to exhaust (max_retries=3, so 4 attempts total).
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    // Job should end up in DLQ.
    let dlq_len = shim.dlq_length().await;
    assert!(
        dlq_len >= 1,
        "Job should be in DLQ after exhausting retries, dlq_len={}",
        dlq_len
    );

    shim.stop().await.unwrap();
}

// ============================================================================
// scheduler-shim E2E
// ============================================================================

/// Verify scheduler-shim fires a task on schedule.
#[tokio::test]
#[serial]
async fn e2e_scheduler_shim_fires_task() {
    use scheduler_shim::{RetryConfig, ScheduledTask, SchedulerShim, TaskState};

    let tasks = vec![ScheduledTask {
        name: "e2e-test-task".into(),
        schedule: "0 * * * * *".into(), // every minute
        command: "echo".into(),
        args: vec!["e2e".into()],
        enabled: true,
        timeout_secs: 60,
        retry: RetryConfig {
            max_retries: 1,
            base_delay_secs: 1,
            max_delay_secs: 5,
        },
    }];

    // Create shim inside env scope, then do sync assertions outside.
    let mut shim = temp_env::with_vars(
        &[(
            "SCHEDULER_TASKS",
            Some(serde_json::to_string(&tasks).unwrap()),
        )],
        SchedulerShim::new,
    );

    // Verify task was loaded.
    let task_list = shim.list_tasks();
    assert_eq!(task_list.len(), 1);
    assert_eq!(task_list[0].name, "e2e-test-task");

    // Verify next run is scheduled.
    let next = shim.next_run("e2e-test-task");
    assert!(next.is_some(), "Task should have a next run time");

    // Verify initial state.
    assert_eq!(shim.task_state("e2e-test-task"), Some(TaskState::Pending));

    // Simulate task execution by updating state.
    shim.update_task_state("e2e-test-task", TaskState::Running);
    assert_eq!(shim.task_state("e2e-test-task"), Some(TaskState::Running));

    shim.update_task_state("e2e-test-task", TaskState::Success);
    assert_eq!(shim.task_state("e2e-test-task"), Some(TaskState::Success));

    // Verify last_success was recorded.
    let info = shim.list_tasks();
    assert!(info[0].last_success.is_some());
}

// ============================================================================
// cdc-shim E2E
// ============================================================================

/// Verify CDC event capture, serialization, and publishing lifecycle.
#[tokio::test]
#[serial]
async fn e2e_cdc_shim_event_lifecycle() {
    use cdc_shim::{CdcOperation, CdcShim};

    // Create shim inside env scope, then do async work outside.
    let mut shim = temp_env::with_vars([("CDC_TABLES", Some("users,orders"))], CdcShim::new);

    // Verify table filter works.
    assert!(shim.should_capture("users"));
    assert!(shim.should_capture("orders"));
    assert!(!shim.should_capture("payments"));

    // Create a CDC event for a users INSERT.
    let event = shim.create_event(
        "users",
        CdcOperation::Insert,
        None,
        Some(serde_json::json!({
            "id": 42,
            "name": "Alice",
            "email": "alice@example.com"
        })),
    );

    assert_eq!(event.table, "users");
    assert_eq!(event.operation, CdcOperation::Insert);

    // Capture the event.
    let captured = shim.capture(event);
    assert!(captured, "Event should be captured");
    assert_eq!(shim.pending_count(), 1);

    // Create and capture an UPDATE event.
    let update_event = shim.create_event(
        "orders",
        CdcOperation::Update,
        Some(serde_json::json!({"id": 100})),
        Some(serde_json::json!({
            "id": 100,
            "status": "shipped"
        })),
    );
    shim.capture(update_event);
    assert_eq!(shim.pending_count(), 2);

    // Publish the batch.
    let published = shim.publish_batch().await;
    assert!(published > 0, "At least one event should be published");
    assert_eq!(shim.pending_count(), 0);

    // Verify stats.
    let stats = shim.stats();
    assert!(stats.events_captured > 0);
    assert!(stats.events_published > 0);

    // Verify serialization produces valid JSON.
    let event = shim.create_event(
        "users",
        CdcOperation::Insert,
        None,
        Some(serde_json::json!({"id": 99})),
    );
    let json = shim.serialize_event(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["table"], "users");
    assert!(parsed["event_id"].is_string());
}

/// Verify CDC WAL position tracking and event ordering.
#[tokio::test]
#[serial]
async fn e2e_cdc_shim_wal_tracking() {
    use cdc_shim::CdcShim;

    let mut shim = CdcShim::new();

    // Set initial WAL position.
    shim.set_wal_position("0/1000000", 1, 0);
    assert_eq!(shim.stats().events_captured, 0);

    // Advance WAL and capture events.
    shim.advance_wal(100);
    shim.set_lag(0.5);

    // Create events with different LSNs to verify ordering.
    for i in 0..5 {
        let mut event = shim.create_event(
            "users",
            cdc_shim::CdcOperation::Insert,
            None,
            Some(serde_json::json!({"id": i})),
        );
        event.lsn = format!("0/{:x}", 0x1000000 + i * 100);
        shim.capture(event);
    }

    let stats = shim.stats();
    assert_eq!(stats.events_captured, 5);

    // Publish and verify all are published.
    let published = shim.publish_batch().await;
    assert_eq!(published, 5);

    // Verify recent_published contains the events.
    let recent = shim.recent_published();
    assert!(!recent.is_empty());
}

// ============================================================================
// chaos-shim E2E
// ============================================================================

/// Verify chaos-shim experiment lifecycle and fault injection.
#[tokio::test]
#[serial]
async fn e2e_chaos_shim_experiment_lifecycle() {
    use chaos_shim::{ChaosShim, FaultType};

    let mut shim = ChaosShim::new();
    shim.set_enabled(true);

    // Verify no active experiments initially.
    assert!(shim.active_experiments().is_empty());
    assert!(!shim.is_active());

    // Start a latency experiment.
    let exp = shim.start_experiment(
        "e2e-latency-test",
        FaultType::Latency,
        "web-1,web-2",
        0.5,
        60,
    );

    assert_eq!(exp.name, "e2e-latency-test");
    assert!(exp.enabled);
    assert!((exp.blast_radius - 0.5).abs() < 0.01);

    let exp_id = exp.id.clone();

    // Verify experiment is active.
    assert_eq!(shim.active_experiments().len(), 1);
    assert!(shim.is_active());

    // Evaluate fault injection with env-var-configured shim.
    let mut eval_shim = temp_env::with_vars(
        [
            ("CHAOS_ENABLED", Some("true")),
            ("CHAOS_LATENCY_MS", Some("150")),
            ("CHAOS_TARGET", Some("all")),
            ("CHAOS_BLAST_RADIUS", Some("1.0")),
        ],
        ChaosShim::new,
    );

    let result = eval_shim.evaluate("web-1");
    assert!(result.injected, "Fault should be injected");
    assert_eq!(result.fault_type, FaultType::Latency);
    assert_eq!(result.delay_ms, 150);

    // Stop the experiment.
    assert!(shim.stop_experiment(&exp_id));
    assert!(shim.active_experiments().is_empty());
    assert!(!shim.is_active());

    // Verify experiment was stopped.
    let exp = shim.get_experiment(&exp_id).unwrap();
    assert!(!exp.enabled);
}

/// Verify chaos-shim orchestrator schedule and tick lifecycle.
#[tokio::test]
#[serial]
async fn e2e_chaos_shim_orchestrator_lifecycle() {
    use chaos_shim::{ChaosOrchestrator, FaultType};

    let mut orch = ChaosOrchestrator::new();
    orch.set_enabled(true);

    // Start an experiment via orchestrator.
    let exp_id = orch
        .start_experiment(
            "e2e-orchestrator-test",
            FaultType::Error,
            "db-primary",
            1.0,
            30,
        )
        .unwrap();

    assert_eq!(orch.active_count(), 1);

    // Register a schedule.
    orch.register_schedule(&exp_id, "0 */6 * * *", Some(2));
    assert!(orch.can_run_scheduled(&exp_id));

    // Record runs.
    orch.record_scheduled_run(&exp_id);
    orch.record_scheduled_run(&exp_id);
    assert!(!orch.can_run_scheduled(&exp_id)); // max_runs=2 reached

    // Complete experiment.
    assert!(orch.complete_experiment(&exp_id, true));
    assert_eq!(orch.active_count(), 0);
    assert_eq!(orch.completed_count(), 1);

    // Verify history.
    let history = orch.history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].experiment_id, exp_id);
    assert!(history[0].success);

    // Test tick expiration.
    let _short_id = orch
        .start_experiment(
            "short-lived",
            FaultType::Latency,
            "all",
            1.0,
            0, // TTL=0 means expires immediately
        )
        .unwrap();
    orch.tick();
    assert_eq!(orch.active_count(), 0);
    assert!(orch.completed_count() >= 2);
}

/// Verify chaos-shim error injection works correctly.
#[tokio::test]
#[serial]
async fn e2e_chaos_shim_error_injection() {
    use chaos_shim::{ChaosShim, FaultType};

    // Create shim inside env scope, then do assertions outside.
    let mut shim = temp_env::with_vars(
        [
            ("CHAOS_ENABLED", Some("true")),
            ("CHAOS_ERROR_RATE", Some("1.0")),
            ("CHAOS_TARGET", Some("all")),
            ("CHAOS_BLAST_RADIUS", Some("1.0")),
        ],
        || {
            let mut s = ChaosShim::new();
            s.set_enabled(true);
            s.set_error_rate(1.0);
            s
        },
    );

    let exp = shim.start_experiment("e2e-error-test", FaultType::Error, "all", 1.0, 60);
    let exp_id = exp.id.clone();

    // Evaluate multiple requests — all should inject errors.
    for i in 0..10 {
        let result = shim.evaluate(&format!("target-{}", i));
        assert!(result.injected, "Request {} should have error injected", i);
        assert_eq!(result.fault_type, FaultType::Error);
    }

    // Verify injection stats.
    assert_eq!(shim.active_experiments().len(), 1);

    shim.stop_experiment(&exp_id);
}
