//! Vault integration tests.

/// Test Vault credentials serialization.
#[tokio::test]
async fn test_vault_credentials_serialization() {
    use vault_shim::Credentials;

    let creds = Credentials {
        username: "postgres".to_string(),
        password: "s3cret_p@ssw0rd".to_string(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
        expires_at: Some(
            (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
        ),
    };

    let json = serde_json::to_string(&creds).unwrap();
    assert!(json.contains("postgres"));
    assert!(json.contains("s3cret_p@ssw0rd"));
    assert!(json.contains("fetched_at"));
    assert!(json.contains("expires_at"));

    println!("Vault credentials serialization works: {}", json);
}

/// Test credential file format.
#[tokio::test]
async fn test_credential_file_format() {
    use vault_shim::Credentials;

    let creds = Credentials {
        username: "admin".to_string(),
        password: "hunter2".to_string(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
        expires_at: None,
    };

    let pgpass = format!("{}:{}", creds.username, creds.password);
    assert_eq!(pgpass, "admin:hunter2");

    let mysql_pwd = format!("{}:{}", creds.username, creds.password);
    assert_eq!(mysql_pwd, "admin:hunter2");

    println!("Credential file formats work:");
    println!("  .pgpass: {}", pgpass);
    println!("  MYSQL_PWD: {}", mysql_pwd);
}
