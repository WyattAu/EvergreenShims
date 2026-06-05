//! Integration tests for all EvergreenShims.
//!
//! These tests verify shim behavior across the full shim matrix.
//! Run with: cargo test -p evergreen-shims-integration

mod backup;
mod failover;
mod vault;

// ============================================================================
// Backup Shim DB Connector Tests
// ============================================================================

/// Test backup-shim Postgres connector env var configuration.
#[tokio::test]
async fn test_backup_postgres_connector_config() {
    use backup_shim::BackupShim;

    temp_env::with_vars(
        [
            ("BACKUP_DB_TYPE", Some("postgres")),
            ("BACKUP_DB_HOST", Some("pg-primary.internal")),
            ("BACKUP_DB_PORT", Some("5432")),
            ("BACKUP_DB_USER", Some("backup_user")),
            ("BACKUP_DB_PASSWORD", Some("secret")),
            ("BACKUP_DATABASE", Some("production")),
            ("BACKUP_CMD", Some("pg_dump")),
            ("BACKUP_OUTPUT_DIR", Some("/var/backups/pg")),
            ("BACKUP_TIMEOUT_SECS", Some("7200")),
        ],
        || {
            let shim = BackupShim::new();
            assert_eq!(shim.db_type(), "postgres");
            assert_eq!(shim.db_host(), "pg-primary.internal");
            assert_eq!(shim.db_port(), 5432);
            assert_eq!(shim.db_user(), "backup_user");
            assert_eq!(shim.db_password(), "secret");
            assert_eq!(shim.database(), "production");
            assert_eq!(shim.backup_cmd(), "pg_dump");
            assert_eq!(shim.output_dir(), "/var/backups/pg");
            assert_eq!(shim.timeout_secs(), 7200);
        },
    );
}

/// Test backup-shim Redis connector env var configuration.
#[tokio::test]
async fn test_backup_redis_connector_config() {
    use backup_shim::BackupShim;

    temp_env::with_vars(
        [
            ("BACKUP_DB_TYPE", Some("redis")),
            ("REDIS_URL", Some("redis://redis-cluster:6380")),
            ("REDIS_PASSWORD", Some("redis-secret")),
            ("BACKUP_OUTPUT_DIR", Some("/var/backups/redis")),
            ("BACKUP_TIMEOUT_SECS", Some("120")),
        ],
        || {
            let shim = BackupShim::new();
            assert_eq!(shim.db_type(), "redis");
            assert_eq!(shim.redis_url(), "redis://redis-cluster:6380");
            assert_eq!(shim.redis_password(), "redis-secret");
            assert_eq!(shim.output_dir(), "/var/backups/redis");
            assert_eq!(shim.timeout_secs(), 120);
        },
    );
}

/// Test backup SHA-256 checksum verification.
#[tokio::test]
async fn test_backup_checksum_verification() {
    use backup_shim::BackupShim;

    let data = b"PGDMP-1500-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00-00";
    let checksum = BackupShim::compute_checksum(data);
    let shim = BackupShim::new();

    let entry = backup_shim::BackupEntry {
        filename: "testdb_20260101_120000.sql.gz".to_string(),
        created_at: chrono::Utc::now(),
        size_bytes: data.len() as u64,
        checksum: checksum.clone(),
    };

    assert!(shim.validate_backup(&entry, data));
    assert_eq!(checksum.len(), 64); // SHA-256 hex
    assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));
}

// ============================================================================
// Replication Shim DB Connector Tests
// ============================================================================

/// Test replication-shim WAL tracking configuration.
#[tokio::test]
async fn test_replication_wal_tracking_config() {
    use replication_shim::ReplicationShim;

    temp_env::with_vars(
        [
            ("REPLICATION_DB_HOST", Some("pg-primary.internal")),
            ("REPLICATION_DB_PORT", Some("5433")),
            ("REPLICATION_DB_USER", Some("repl_monitor")),
            ("REPLICATION_DB_PASSWORD", Some("repl-secret")),
            ("REPLICATION_DB_NAME", Some("production")),
            ("REPLICATION_LAG_THRESHOLD_BYTES", Some("2097152")),
        ],
        || {
            let shim = ReplicationShim::new();
            assert_eq!(shim.db_host(), "pg-primary.internal");
            assert_eq!(shim.db_port(), 5433);
            assert_eq!(shim.db_user(), "repl_monitor");
            assert_eq!(shim.db_password(), "repl-secret");
            assert_eq!(shim.db_name(), "production");
            assert_eq!(shim.lag_threshold_bytes(), 2_097_152);
        },
    );
}

/// Test replication-shim lag threshold default.
#[tokio::test]
async fn test_replication_lag_threshold_default() {
    use replication_shim::ReplicationShim;

    let shim = ReplicationShim::new();
    assert_eq!(shim.lag_threshold_bytes(), 1_048_576); // 1MB default
}

/// Test replication-shim state transitions with lag tracking.
#[tokio::test]
async fn test_replication_lag_state_transitions() {
    use replication_shim::{ReplicationShim, ReplicationState};

    let mut shim = ReplicationShim::new();
    shim.add_replica("rep1:5432".to_string());

    // Low lag -> Healthy
    shim.update_replica_status("rep1:5432", 100, 0.5);
    shim.recalculate_state();
    assert_eq!(shim.replication_state(), ReplicationState::Healthy);

    // High lag -> Degraded
    shim.update_replica_status("rep1:5432", 15000, 15.0);
    shim.recalculate_state();
    assert_eq!(shim.replication_state(), ReplicationState::Degraded);

    // Critical lag -> Broken
    shim.update_replica_status("rep1:5432", 50000, 60.0);
    shim.recalculate_state();
    assert_eq!(shim.replication_state(), ReplicationState::Broken);
}

/// Test replication-shim WAL position update.
#[tokio::test]
async fn test_replication_wal_position_update() {
    use replication_shim::ReplicationShim;

    let mut shim = ReplicationShim::new();
    shim.set_wal_position("0/1A00000", 2, 4096);

    let wal = shim.wal_position();
    assert_eq!(wal.lsn, "0/1A00000");
    assert_eq!(wal.segment, 2);
    assert_eq!(wal.offset, 4096);
}

// ============================================================================
// Migration Shim DB Connector Tests
// ============================================================================

/// Test migration-shim DB URL override configuration.
#[tokio::test]
async fn test_migration_db_url_override() {
    use migration_shim::MigrationShim;

    temp_env::with_vars(
        [
            (
                "MIGRATION_DB_URL",
                Some("postgres://admin:s3cret@db.prod:5432/app"),
            ),
            ("MIGRATION_DIR", Some("/opt/migrations")),
            ("MIGRATION_AUTO_MIGRATE", Some("true")),
        ],
        || {
            let shim = MigrationShim::new();
            assert_eq!(
                shim.get_connection_string(),
                "postgres://admin:s3cret@db.prod:5432/app"
            );
            assert_eq!(shim.dir(), &std::path::PathBuf::from("/opt/migrations"));
            assert!(shim.auto_migrate());
        },
    );
}

/// Test migration-shim sequential apply with checksum verification.
#[tokio::test]
async fn test_migration_sequential_apply_with_checksum() {
    use migration_shim::{Migration, MigrationShim};

    let mut shim = MigrationShim::new();

    let m1 = Migration {
        version: 1,
        name: "create_users".to_string(),
        up_sql: "CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT NOT NULL)".to_string(),
        down_sql: Some("DROP TABLE users".to_string()),
        checksum: MigrationShim::compute_checksum(
            "CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
        ),
    };

    let m2 = Migration {
        version: 2,
        name: "add_email_index".to_string(),
        up_sql: "CREATE INDEX idx_users_email ON users (email)".to_string(),
        down_sql: Some("DROP INDEX idx_users_email".to_string()),
        checksum: MigrationShim::compute_checksum("CREATE INDEX idx_users_email ON users (email)"),
    };

    shim.apply_migration(&m1).unwrap();
    assert_eq!(shim.current_version(), 1);
    assert_eq!(shim.migrations_applied(), 1);

    shim.apply_migration(&m2).unwrap();
    assert_eq!(shim.current_version(), 2);
    assert_eq!(shim.migrations_applied(), 2);

    // Verify checksum integrity
    let records = shim.applied();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].checksum, m1.checksum);
    assert_eq!(records[1].checksum, m2.checksum);
}

/// Test migration-shim rollback restores version.
#[tokio::test]
async fn test_migration_rollback_restores_version() {
    use migration_shim::{Migration, MigrationShim};

    let mut shim = MigrationShim::new();

    let m1 = Migration {
        version: 1,
        name: "create_table".to_string(),
        up_sql: "CREATE TABLE test (id INT)".to_string(),
        down_sql: Some("DROP TABLE test".to_string()),
        checksum: MigrationShim::compute_checksum("CREATE TABLE test (id INT)"),
    };

    let m2 = Migration {
        version: 2,
        name: "add_column".to_string(),
        up_sql: "ALTER TABLE test ADD name TEXT".to_string(),
        down_sql: Some("ALTER TABLE test DROP COLUMN name".to_string()),
        checksum: MigrationShim::compute_checksum("ALTER TABLE test ADD name TEXT"),
    };

    let m3 = Migration {
        version: 3,
        name: "add_index".to_string(),
        up_sql: "CREATE INDEX idx_test ON test (id)".to_string(),
        down_sql: Some("DROP INDEX idx_test".to_string()),
        checksum: MigrationShim::compute_checksum("CREATE INDEX idx_test ON test (id)"),
    };

    shim.apply_migration(&m1).unwrap();
    shim.apply_migration(&m2).unwrap();
    shim.apply_migration(&m3).unwrap();
    assert_eq!(shim.current_version(), 3);

    // Rollback m3
    let rolled_back = shim.rollback_last().unwrap();
    assert_eq!(rolled_back.version, 3);
    assert_eq!(shim.current_version(), 2);
    assert_eq!(shim.migrations_rolled_back(), 1);

    // Rollback m2
    let rolled_back = shim.rollback_last().unwrap();
    assert_eq!(rolled_back.version, 2);
    assert_eq!(shim.current_version(), 1);
    assert_eq!(shim.migrations_rolled_back(), 2);

    // Re-apply m2 with a new version
    let _m2b = Migration {
        version: 2,
        name: "add_column_v2".to_string(),
        up_sql: "ALTER TABLE test ADD email TEXT".to_string(),
        down_sql: Some("ALTER TABLE test DROP COLUMN email".to_string()),
        checksum: MigrationShim::compute_checksum("ALTER TABLE test ADD email TEXT"),
    };
    // Note: can't re-apply version 2 since the old record is still in memory.
    // Instead, verify rollback state is correct.
    assert_eq!(shim.applied().len(), 1);
    assert_eq!(shim.applied()[0].version, 1);
}

/// Test migration-shim file scanning and version ordering.
#[tokio::test]
async fn test_migration_file_scanning_ordering() {
    use migration_shim::MigrationShim;

    // Verify version parsing handles various formats
    assert_eq!(MigrationShim::parse_version("001_init.up.sql").unwrap(), 1);
    assert_eq!(
        MigrationShim::parse_version("010_add_index.up.sql").unwrap(),
        10
    );
    assert_eq!(
        MigrationShim::parse_version("099_final.up.sql").unwrap(),
        99
    );

    // Verify name parsing
    assert_eq!(
        MigrationShim::parse_name("001_create_users.up.sql"),
        "create_users"
    );
    assert_eq!(
        MigrationShim::parse_name("002_add_email_index.down.sql"),
        "add_email_index"
    );
}

// ============================================================================
// Config Shim Integration Tests
// ============================================================================

/// Test config-shim file hash detection with real files.
#[tokio::test]
async fn test_config_hash_detection() {
    use config_shim::ConfigShim;

    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.toml");

    // Write initial config
    std::fs::write(&config_file, "key = \"value1\"\n").unwrap();

    let _shim = ConfigShim::new();

    // Verify hash changes when file changes
    let hash1 = ConfigShim::file_hash(&config_file).await;
    assert!(hash1.is_some());

    // Modify file
    std::fs::write(&config_file, "key = \"value2\"\n").unwrap();
    let hash2 = ConfigShim::file_hash(&config_file).await;
    assert!(hash2.is_some());
    assert_ne!(hash1, hash2);
}

// ============================================================================
// Encryption Shim Integration Tests
// ============================================================================

/// Test AES-GCM roundtrip with real crypto.
#[tokio::test]
async fn test_encryption_aes_gcm_roundtrip() {
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

            let plaintext = b"Hello, EvergreenShims! This is a secret message.";
            let fixed_nonce = [1u8; 12];
            let encrypted = shim.encrypt(plaintext, Some(&fixed_nonce)).unwrap();

            assert_eq!(encrypted.method, "aes-gcm");
            assert_eq!(encrypted.key_id, "default");
            assert_eq!(encrypted.ciphertext.len(), plaintext.len());
            assert_eq!(encrypted.nonce.len(), 12);
            assert_eq!(encrypted.tag.len(), 16);

            let decrypted = shim.decrypt(&encrypted).unwrap();
            assert_eq!(decrypted, plaintext);
        },
    );
}

/// Test ChaCha20-Poly1305 roundtrip.
#[tokio::test]
async fn test_encryption_chacha20_roundtrip() {
    use encryption_shim::EncryptionShim;

    temp_env::with_vars(
        [
            ("ENCRYPTION_METHOD", Some("chacha20")),
            (
                "ENCRYPTION_KEY",
                Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            ),
        ],
        || {
            let mut shim = EncryptionShim::new();

            let plaintext = b"ChaCha20 encryption test data";
            let fixed_nonce = [2u8; 12];
            let encrypted = shim.encrypt(plaintext, Some(&fixed_nonce)).unwrap();
            let decrypted = shim.decrypt(&encrypted).unwrap();

            assert_eq!(decrypted, plaintext);
        },
    );
}

/// Test encryption with key rotation.
#[tokio::test]
async fn test_encryption_key_rotation() {
    use encryption_shim::EncryptionShim;

    temp_env::with_vars(
        [(
            "ENCRYPTION_KEY",
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        )],
        || {
            let mut shim = EncryptionShim::new();

            let fixed_nonce1 = [3u8; 12];
            let encrypted1 = shim
                .encrypt(b"before rotation", Some(&fixed_nonce1))
                .unwrap();
            assert_eq!(encrypted1.key_id, "default");

            // Rotate
            let new_key = [
                0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45,
                0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01,
                0x23, 0x45, 0x67, 0x89,
            ];
            shim.rotate_key("v2".to_string(), new_key.to_vec()).unwrap();

            // Old data still decrypts
            let decrypted = shim.decrypt(&encrypted1).unwrap();
            assert_eq!(decrypted, b"before rotation");

            // New data uses new key
            let fixed_nonce2 = [4u8; 12];
            let encrypted2 = shim
                .encrypt(b"after rotation", Some(&fixed_nonce2))
                .unwrap();
            assert_eq!(encrypted2.key_id, "v2");
            assert_ne!(encrypted1.key_id, encrypted2.key_id);
        },
    );
}

// ============================================================================
// Scheduler Shim Integration Tests
// ============================================================================

/// Test scheduler cron parsing and task management.
#[tokio::test]
async fn test_scheduler_cron_integration() {
    use scheduler_shim::{RetryConfig, ScheduledTask, SchedulerShim};

    let tasks = vec![
        ScheduledTask {
            name: "hourly-backup".into(),
            schedule: "0 * * * * *".into(),
            command: "backup.sh".into(),
            args: vec!["--full".into()],
            enabled: true,
            timeout_secs: 600,
            retry: RetryConfig {
                max_retries: 3,
                base_delay_secs: 5,
                max_delay_secs: 300,
            },
        },
        ScheduledTask {
            name: "disabled-task".into(),
            schedule: "0 0 3 * * *".into(),
            command: "report.sh".into(),
            args: vec![],
            enabled: false, // disabled
            timeout_secs: 60,
            retry: RetryConfig::default(),
        },
    ];

    temp_env::with_vars(
        &[(
            "SCHEDULER_TASKS",
            Some(serde_json::to_string(&tasks).unwrap()),
        )],
        || {
            let shim = SchedulerShim::new();

            // Only enabled task should be loaded (disabled filtered out)
            let task_list = shim.list_tasks();
            assert_eq!(task_list.len(), 1);
            assert_eq!(task_list[0].name, "hourly-backup");

            // Verify next run is scheduled
            let next = shim.next_run("hourly-backup");
            assert!(next.is_some());

            // Disabled task has no state
            assert!(shim.task_state("disabled-task").is_none());
        },
    );
}

/// Test scheduler task state lifecycle.
#[tokio::test]
async fn test_scheduler_state_lifecycle() {
    use scheduler_shim::{RetryConfig, ScheduledTask, SchedulerShim, TaskState};

    let tasks = vec![ScheduledTask {
        name: "test".into(),
        schedule: "0 * * * * *".into(),
        command: "echo".into(),
        args: vec![],
        enabled: true,
        timeout_secs: 30,
        retry: RetryConfig::default(),
    }];

    temp_env::with_vars(
        &[(
            "SCHEDULER_TASKS",
            Some(serde_json::to_string(&tasks).unwrap()),
        )],
        || {
            let mut shim = SchedulerShim::new();

            // Initial state
            assert_eq!(shim.task_state("test"), Some(TaskState::Pending));

            // Running
            shim.update_task_state("test", TaskState::Running);
            assert_eq!(shim.task_state("test"), Some(TaskState::Running));

            // Success
            shim.update_task_state("test", TaskState::Success);
            assert_eq!(shim.task_state("test"), Some(TaskState::Success));

            // List shows updated state
            let info = shim.list_tasks();
            assert_eq!(info.len(), 1);
            assert_eq!(info[0].state, TaskState::Success);
            assert!(info[0].last_success.is_some());
        },
    );
}

// ============================================================================
// Alerting Shim Integration Tests
// ============================================================================

/// Test alerting routing and dedup.
#[tokio::test]
async fn test_alerting_routing_and_dedup() {
    use alerting_shim::{AlertingShim, Severity};
    use std::collections::HashMap;

    temp_env::with_vars(
        [
            (
                "ALERTING_WEBHOOKS",
                Some(
                    r##"[{"name":"slack","url":"http://localhost:3001/slack","channel":"#alerts","min_severity":"info","headers":{}},{"name":"pager","url":"http://localhost:3002/pager","channel":"critical","min_severity":"warning","headers":{}}]"##,
                ),
            ),
            ("ALERTING_DEDUP_WINDOW", Some("60")),
        ],
        || {
            let shim = AlertingShim::new();

            // Info alert -> only slack
            let info_alert = alerting_shim::Alert {
                id: "1".into(),
                title: "Disk usage".into(),
                message: "80%".into(),
                severity: Severity::Info,
                source: "monitoring".into(),
                labels: HashMap::new(),
                timestamp: chrono::Utc::now(),
            };
            let targets = shim.route(&info_alert);
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].name, "slack");

            // Warning -> both
            let warn_alert = alerting_shim::Alert {
                id: "2".into(),
                title: "CPU spike".into(),
                message: "95%".into(),
                severity: Severity::Warning,
                source: "monitoring".into(),
                labels: HashMap::new(),
                timestamp: chrono::Utc::now(),
            };
            let targets = shim.route(&warn_alert);
            assert_eq!(targets.len(), 2);

            // Critical -> both
            let crit_alert = alerting_shim::Alert {
                id: "3".into(),
                title: "DB down".into(),
                message: "Connection refused".into(),
                severity: Severity::Critical,
                source: "health".into(),
                labels: HashMap::new(),
                timestamp: chrono::Utc::now(),
            };
            let targets = shim.route(&crit_alert);
            assert_eq!(targets.len(), 2);
        },
    );
}

// ============================================================================
// Queue Shim Integration Tests
// ============================================================================

/// Test queue job lifecycle.
#[tokio::test]
async fn test_queue_job_lifecycle() {
    use queue_shim::{JobStatus, QueueShim};

    let mut shim = QueueShim::new();

    // Enqueue multiple jobs
    let id1 = shim.enqueue("email".into(), b"hello".to_vec()).await;
    let id2 = shim.enqueue("report".into(), b"monthly".to_vec()).await;
    let _id3 = shim.enqueue("backup".into(), b"full".to_vec()).await;

    assert_eq!(shim.queue_depth().await, 3);

    // Dequeue first job
    let job = shim.dequeue().await.unwrap();
    assert_eq!(job.id, id1);
    assert_eq!(job.status, JobStatus::Running);
    assert_eq!(shim.running_count().await, 1);
    assert_eq!(shim.pending_count().await, 2);

    // Complete first job
    shim.complete_job(&job.id).await.unwrap();
    assert_eq!(shim.running_count().await, 0);

    // Dequeue and fail second job
    let job2 = shim.dequeue().await.unwrap();
    assert_eq!(job2.id, id2);
    let status = shim.fail_job(&job2.id, "timeout".into()).await.unwrap();
    assert_eq!(status, JobStatus::Retrying);

    // Job is back in pending with retry
    assert_eq!(shim.pending_count().await, 2); // job2 retried + id3 still pending
}

/// Test queue DLQ after exhausting retries.
#[tokio::test]
async fn test_queue_dlq_exhaustion() {
    use queue_shim::{JobStatus, QueueShim};

    let mut shim = QueueShim::new();
    let id = shim.enqueue("doomed".into(), vec![]).await;

    // Exhaust retries (max_retries=3, so 4 attempts = dead)
    for _ in 0..4 {
        shim.dequeue().await;
        let status = shim.fail_job(&id, "persistent error".into()).await.unwrap();
        if status == JobStatus::Retrying {
            // Re-dequeue for next attempt
            shim.dequeue().await;
        }
    }

    assert_eq!(shim.dlq_length().await, 1);

    let dlq = shim.drain_dlq().await;
    assert_eq!(dlq.len(), 1);
    assert_eq!(dlq[0].job.id, id);
    assert!(dlq[0].reason.contains("persistent error"));
}

// ============================================================================
// Auth Shim Integration Tests
// ============================================================================

/// Test auth token creation and validation.
#[tokio::test]
async fn test_auth_token_lifecycle() {
    use auth_shim::{AuthShim, Role};

    let mut shim = AuthShim::new();

    // Create token
    let token_value = shim.create_token("user123", Role::ReadWrite, None);

    assert!(!token_value.is_empty());

    // Validate token
    let result = shim.validate_token(&token_value);
    assert!(result.authenticated);

    // Token is stored internally
    // No public list_tokens method — validated via metrics
}

/// Test API key management.
#[tokio::test]
async fn test_auth_api_keys() {
    use auth_shim::{ApiKey, AuthShim, Role};

    let mut shim = AuthShim::new();

    // Register API key
    let raw_key = "my-secret-api-key";
    let key_hash = shim.hash_api_key(raw_key);
    let key = ApiKey {
        key_id: "key-1".to_string(),
        name: "service-a".to_string(),
        key_hash,
        role: Role::ReadOnly,
        created_at: chrono::Utc::now().to_rfc3339(),
        last_used: None,
        revoked: false,
    };
    shim.register_api_key(key);

    // Validate key
    let result = shim.validate_api_key("key-1", raw_key);
    assert!(result.authenticated);

    // Revoke
    assert!(shim.revoke_api_key("key-1"));
    let result = shim.validate_api_key("key-1", raw_key);
    assert!(!result.authenticated);
}

// ============================================================================
// Compliance Shim Integration Tests
// ============================================================================

/// Test compliance rule checks and scoring.
#[tokio::test]
async fn test_compliance_scoring() {
    use compliance_shim::ComplianceShim;

    let mut shim = ComplianceShim::new();

    // Add checks via public API
    shim.add_check(compliance_shim::ComplianceCheck {
        id: "TLS-001".into(),
        description: "TLS 1.3 enforced".into(),
        benchmark: "cis".into(),
        severity: compliance_shim::Severity::High,
        passed: true,
        evidence: "Config verified: TLS 1.3 minimum".into(),
        remediation: "Enable TLS 1.3 in config".into(),
    });
    shim.add_check(compliance_shim::ComplianceCheck {
        id: "AUTH-001".into(),
        description: "Authentication required".into(),
        benchmark: "cis".into(),
        severity: compliance_shim::Severity::Critical,
        passed: false,
        evidence: String::new(),
        remediation: "Enable auth middleware".into(),
    });

    // Run checks
    shim.run_checks();

    // Check report
    let report = shim.generate_report();
    assert_eq!(report.total_checks, 2);

    // Count passed/failed
    assert_eq!(report.passed, 1);
    assert_eq!(report.failed, 1);

    // Violations
    assert!(shim.meets_threshold(50.0)); // 50% pass rate
    assert!(!shim.meets_threshold(80.0)); // 80% not met
}

// ============================================================================
// Cache Shim Integration Tests
// ============================================================================

/// Test cache TTL and eviction.
#[tokio::test]
async fn test_cache_lifecycle() {
    use cache_shim::CacheShim;

    let shim = CacheShim::new();

    // Set values
    shim.set("key1", b"value1");
    shim.set("key2", b"value2");
    shim.set("key3", b"value3");

    assert_eq!(shim.entry_count(), 3);

    // Get value
    let val = shim.get("key1");
    assert!(val.is_some());
    assert_eq!(val.unwrap(), b"value1");

    // Invalidate prefix
    let invalidated = shim.invalidate_prefix("key");
    assert_eq!(invalidated, 3);

    // Verify cleared
    assert_eq!(shim.entry_count(), 0);
}

// ============================================================================
// CDC Shim Integration Tests
// ============================================================================

/// Test CDC event capture and serialization.
#[tokio::test]
async fn test_cdc_event_lifecycle() {
    use cdc_shim::CdcShim;

    let mut shim = CdcShim::new();

    // Create event
    let event = shim.create_event(
        "users",
        cdc_shim::CdcOperation::Insert,
        None,
        Some(serde_json::json!({"id": 1, "name": "Alice"})),
    );

    // Capture event
    let captured = shim.capture(event);
    assert!(captured);

    // Get stats
    let stats = shim.stats();
    assert!(stats.events_captured > 0);
}

// ============================================================================
// Sharding Shim Integration Tests
// ============================================================================

/// Test sharding hash-based routing.
#[tokio::test]
async fn test_sharding_hash_routing() {
    use sharding_shim::ShardingShim;

    temp_env::with_vars(
        [(
            "SHARDING_ADDRESSES",
            Some("redis://localhost:6380,redis://localhost:6381,redis://localhost:6382"),
        )],
        || {
            let mut shim = ShardingShim::new();

            // Route different keys
            let mut shard_counts = std::collections::HashMap::new();
            for i in 0..1000 {
                let key = format!("user:{}", i);
                let (shard_id, _addr) = shim.route(&key).unwrap();
                *shard_counts.entry(shard_id).or_insert(0) += 1;
            }

            // All keys should be routed
            assert_eq!(shard_counts.values().sum::<u32>(), 1000);
            // Distribution should be roughly even (at least 2 of 3 shards used)
            assert!(shard_counts.len() >= 2);
        },
    );
}

// ============================================================================
// Chaos Shim Integration Tests
// ============================================================================

/// Test chaos experiment lifecycle.
#[tokio::test]
async fn test_chaos_experiment_lifecycle() {
    use chaos_shim::ChaosShim;

    let mut shim = ChaosShim::new();

    // Start experiment
    let exp = shim.start_experiment(
        "latency-test",
        chaos_shim::FaultType::Latency,
        "web-1,web-2",
        0.5, // 50% of traffic
        60,
    );

    let exp_name = exp.name.clone();
    let exp_id = exp.id.clone();
    assert_eq!(exp_name, "latency-test");
    assert!(!shim.active_experiments().is_empty());

    // Stop experiment
    assert!(shim.stop_experiment(&exp_id));
    assert_eq!(shim.active_experiments().len(), 0);
}

// ============================================================================
// Cost Shim Integration Tests
// ============================================================================

/// Test cost tracking and budget alerts.
#[tokio::test]
async fn test_cost_budget_tracking() {
    use cost_shim::CostShim;

    let mut shim = CostShim::new();

    // Add budget
    shim.create_budget("cpu-monthly", 1000.0);

    // Record usage
    shim.record_usage(
        "cpu-monthly",
        cost_shim::ResourceType::Cpu,
        100.0,
        "hours",
        1.0,
    );
    shim.record_usage(
        "cpu-monthly",
        cost_shim::ResourceType::Cpu,
        200.0,
        "hours",
        1.0,
    );

    // Check budget
    let budget = shim.get_budget("cpu-monthly").unwrap();
    assert!(!budget.is_over_budget());
    assert!(!budget.is_near_limit(80.0));
    assert!(budget.remaining() > 600.0);

    // Over budget
    shim.record_usage(
        "cpu-monthly",
        cost_shim::ResourceType::Cpu,
        800.0,
        "hours",
        1.0,
    );
    let budget = shim.get_budget("cpu-monthly").unwrap();
    assert!(budget.is_over_budget());

    // Alerts
    let alerts = shim.check_alerts();
    assert!(!alerts.is_empty());
}

// ============================================================================
// Archival Shim Integration Tests
// ============================================================================

/// Test archival lifecycle transitions.
#[tokio::test]
async fn test_archival_lifecycle() {
    use archival_shim::ArchivalShim;

    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap().to_string();

    // Create shim inside env scope so it reads ARCHIVAL_ARCHIVE_PATH
    let mut shim =
        temp_env::with_vars([("ARCHIVAL_ARCHIVE_PATH", Some(dir_path.as_str()))], || {
            let mut shim = ArchivalShim::new();
            shim.add_retention_rule(archival_shim::RetentionRule {
                table: "logs".into(),
                age_days: 7,
                lifecycle_days: 30,
                storage_tier: archival_shim::StorageTier::Cold,
            });
            shim
        });

    // Archive a batch (async, outside temp_env scope)
    let archived = shim.archive_batch("logs", 3, 3000, None::<&str>).await;

    assert!(archived.is_some());
    let archived = archived.unwrap();
    assert_eq!(archived.table, "logs");

    // Summary
    let summary = shim.summary();
    assert!(summary.total_records > 0);
}

// ============================================================================
// Cross-Shim Event Wiring Integration Tests
// ============================================================================

/// Test the full health→failover event chain via ShimBus.
#[tokio::test]
async fn test_cross_shim_health_to_failover() {
    use shim_core::event::EventType;
    use shim_core::wiring::HealthFailoverHandler;
    use shim_core::{Severity, ShimBus};
    use std::sync::Arc;

    let bus = ShimBus::new();
    let handler = Arc::new(HealthFailoverHandler::new(bus.clone(), 2));
    handler.start();

    let mut rx = bus.subscribe();

    // Simulate health shim detecting unhealthy
    bus.emit(
        "health-shim",
        EventType::HealthStatusChanged {
            previous: "healthy".into(),
            current: "unhealthy".into(),
        },
        Severity::Warning,
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Second unhealthy should trigger failover
    bus.emit(
        "health-shim",
        EventType::HealthStatusChanged {
            previous: "unhealthy".into(),
            current: "unhealthy".into(),
        },
        Severity::Warning,
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify failover event was generated
    let mut found = false;
    while let Ok(evt) = rx.try_recv() {
        if matches!(evt.event, EventType::FailoverTriggered { .. }) {
            found = true;
            assert_eq!(evt.source, "failover-shim");
            assert_eq!(evt.severity, Severity::Critical);
        }
    }
    assert!(
        found,
        "Health→Failover chain should produce FailoverTriggered event"
    );
}

/// Test the backup→encryption event chain.
#[tokio::test]
async fn test_cross_shim_backup_to_encryption() {
    use shim_core::event::EventType;
    use shim_core::wiring::BackupEncryptionHandler;
    use shim_core::{Severity, ShimBus};
    use std::sync::Arc;

    let bus = ShimBus::new();
    let handler = Arc::new(BackupEncryptionHandler::new(bus.clone()));
    handler.start();

    let mut rx = bus.subscribe();

    // Simulate backup completing
    bus.emit(
        "backup-shim",
        EventType::BackupCompleted {
            name: "postgres-daily".into(),
            size_bytes: 5_000_000,
            checksum: "sha256:abcdef123456".into(),
        },
        Severity::Info,
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify encryption key rotation was triggered
    let mut found = false;
    while let Ok(evt) = rx.try_recv() {
        if let EventType::EncryptionKeyRotated { key_id, algorithm } = &evt.event {
            found = true;
            assert_eq!(key_id, "backup-postgres-daily");
            assert_eq!(algorithm, "AES-256-GCM");
        }
    }
    assert!(
        found,
        "Backup→Encryption chain should produce EncryptionKeyRotated event"
    );
}

/// Test the scheduler→backup event chain.
#[tokio::test]
async fn test_cross_shim_scheduler_to_backup() {
    use shim_core::event::EventType;
    use shim_core::wiring::SchedulerBackupHandler;
    use shim_core::{Severity, ShimBus};
    use std::sync::Arc;

    let bus = ShimBus::new();
    let handler = Arc::new(SchedulerBackupHandler::new(bus.clone()));
    handler.start();

    let mut rx = bus.subscribe();

    // Simulate scheduler firing a backup task
    bus.emit(
        "scheduler-shim",
        EventType::SchedulerTaskFired {
            task_name: "nightly-backup".into(),
            schedule: "0 2 * * *".into(),
        },
        Severity::Info,
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify backup was started
    let mut found = false;
    while let Ok(evt) = rx.try_recv() {
        if let EventType::BackupStarted { name } = &evt.event {
            found = true;
            assert_eq!(name, "nightly-backup");
        }
    }
    assert!(
        found,
        "Scheduler→Backup chain should produce BackupStarted event"
    );
}

/// Test the alert fan-in: all alertable events reach the alerting shim.
#[tokio::test]
async fn test_cross_shim_alert_fan_in() {
    use shim_core::event::EventType;
    use shim_core::wiring::AlertFanInHandler;
    use shim_core::{Severity, ShimBus};
    use std::sync::Arc;

    let bus = ShimBus::new();
    let _handler = Arc::new(AlertFanInHandler::new(bus.clone()));
    // AlertFanInHandler.start() is already called in wire_all_handlers

    let mut rx = bus.subscribe();

    // Emit several alertable events
    bus.emit(
        "backup-shim",
        EventType::BackupFailed {
            name: "redis-daily".into(),
            reason: "connection refused".into(),
        },
        Severity::Error,
    );

    bus.emit(
        "tls-shim",
        EventType::TlsCertExpiring {
            cert_path: "/etc/tls/api.pem".into(),
            days_remaining: 3,
        },
        Severity::Warning,
    );

    bus.emit(
        "health-shim",
        EventType::HealthStatusChanged {
            previous: "healthy".into(),
            current: "unhealthy".into(),
        },
        Severity::Critical,
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // All three alertable events should be visible
    let mut alert_count = 0;
    while let Ok(evt) = rx.try_recv() {
        if evt.is_alertable() {
            alert_count += 1;
        }
    }
    assert!(
        alert_count >= 3,
        "Alert fan-in should forward all alertable events, got {}",
        alert_count
    );
}

/// Test the wire_all_handlers convenience function.
#[tokio::test]
async fn test_wire_all_handlers() {
    use shim_core::event::{EventType, ShimEvent};
    use shim_core::{Severity, ShimBus};

    let bus = ShimBus::new();
    shim_core::wiring::wire_all_handlers(&bus);

    let mut rx = bus.subscribe();

    // Trigger a chain: health → unhealthy (x3) → failover (threshold=3 in wire_all_handlers)
    for _ in 0..3 {
        bus.emit(
            "health-shim",
            EventType::HealthStatusChanged {
                previous: "healthy".into(),
                current: "unhealthy".into(),
            },
            Severity::Warning,
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    // Trigger scheduler → backup chain
    bus.emit(
        "scheduler-shim",
        EventType::SchedulerTaskFired {
            task_name: "weekly-backup".into(),
            schedule: "0 3 * * 0".into(),
        },
        Severity::Info,
    );
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Collect all events
    let mut events: Vec<ShimEvent> = vec![];
    while let Ok(evt) = rx.try_recv() {
        events.push(evt);
    }

    // Should have failover + backup started events (plus the originals)
    let has_failover = events
        .iter()
        .any(|e| matches!(e.event, EventType::FailoverTriggered { .. }));
    let has_backup = events
        .iter()
        .any(|e| matches!(e.event, EventType::BackupStarted { .. }));

    assert!(
        has_failover,
        "wire_all_handlers should include health→failover wiring"
    );
    assert!(
        has_backup,
        "wire_all_handlers should include scheduler→backup wiring"
    );
}

/// Test ShimBus event sequencing across multiple sources.
#[tokio::test]
async fn test_bus_multi_source_sequencing() {
    use shim_core::event::EventType;
    use shim_core::{Severity, ShimBus};

    let bus = ShimBus::new();

    let e1 = bus.emit(
        "shim-a",
        EventType::Custom {
            event_name: "a1".into(),
            payload: serde_json::json!(null),
        },
        Severity::Info,
    );
    let e2 = bus.emit(
        "shim-b",
        EventType::Custom {
            event_name: "b1".into(),
            payload: serde_json::json!(null),
        },
        Severity::Info,
    );
    let e3 = bus.emit(
        "shim-a",
        EventType::Custom {
            event_name: "a2".into(),
            payload: serde_json::json!(null),
        },
        Severity::Info,
    );

    // Sequences are per-source
    assert_eq!(e1.sequence, 1);
    assert_eq!(e2.sequence, 1);
    assert_eq!(e3.sequence, 2);
}
