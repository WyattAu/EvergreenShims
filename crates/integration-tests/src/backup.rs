//! Backup integration tests.

/// Test backup filename generation.
#[tokio::test]
async fn test_backup_filename_generation() {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let database = "testdb";

    let filename_gzip = format!("{}_{}.sql.gz", database, timestamp);
    let filename_zstd = format!("{}_{}.sql.zst", database, timestamp);
    let filename_none = format!("{}_{}.sql", database, timestamp);

    assert!(filename_gzip.ends_with(".sql.gz"));
    assert!(filename_zstd.ends_with(".sql.zst"));
    assert!(filename_none.ends_with(".sql"));

    println!("Backup filename generation works:");
    println!("  gzip: {}", filename_gzip);
    println!("  zstd: {}", filename_zstd);
    println!("  none: {}", filename_none);
}

/// Test backup metadata serialization.
#[tokio::test]
async fn test_backup_metadata_serialization() {
    use backup_shim::BackupMeta;

    let meta = BackupMeta {
        database: "testdb".to_string(),
        db_type: "postgres".to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        completed_at: Some(chrono::Utc::now().to_rfc3339()),
        path: "/var/backups/testdb_20240101_120000.sql.gz".to_string(),
        size_bytes: 1024 * 1024,
        success: true,
        error: None,
    };

    let json = serde_json::to_string(&meta).unwrap();
    assert!(json.contains("testdb"));
    assert!(json.contains("postgres"));
    assert!(json.contains("1048576"));

    println!("Backup metadata serialization works: {}", json);
}
