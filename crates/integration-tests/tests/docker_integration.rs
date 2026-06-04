//! Integration tests with Docker Compose databases.
//!
//! Prerequisites:
//!   docker compose -f tests/docker-compose.yml up -d
//!
//! Run with:
//!   cargo test -p evergreen-shims-integration --test docker_integration

use std::time::Duration;

/// Test PostgreSQL connectivity.
#[tokio::test]
async fn test_postgres_connectivity() {
    use std::net::TcpStream;

    let addr: std::net::SocketAddr = "127.0.0.1:5432".parse().unwrap();
    let result = TcpStream::connect_timeout(&addr, Duration::from_secs(2));

    if result.is_ok() {
        println!("PostgreSQL is reachable at 127.0.0.1:5432");
    } else {
        println!("PostgreSQL not available, skipping test");
        return;
    }
}

/// Test MariaDB connectivity.
#[tokio::test]
async fn test_mariadb_connectivity() {
    use std::net::TcpStream;

    let addr: std::net::SocketAddr = "127.0.0.1:3306".parse().unwrap();
    let result = TcpStream::connect_timeout(&addr, Duration::from_secs(2));

    if result.is_ok() {
        println!("MariaDB is reachable at 127.0.0.1:3306");
    } else {
        println!("MariaDB not available, skipping test");
        return;
    }
}

/// Test Redis connectivity.
#[tokio::test]
async fn test_redis_connectivity() {
    use std::net::TcpStream;

    let addr: std::net::SocketAddr = "127.0.0.1:6379".parse().unwrap();
    let result = TcpStream::connect_timeout(&addr, Duration::from_secs(2));

    if result.is_ok() {
        println!("Redis is reachable at 127.0.0.1:6379");
    } else {
        println!("Redis not available, skipping test");
        return;
    }
}

/// Test Vault connectivity.
#[tokio::test]
async fn test_vault_connectivity() {
    use std::net::TcpStream;

    let addr: std::net::SocketAddr = "127.0.0.1:8200".parse().unwrap();
    let result = TcpStream::connect_timeout(&addr, Duration::from_secs(2));

    if result.is_ok() {
        println!("Vault is reachable at 127.0.0.1:8200");
    } else {
        println!("Vault not available, skipping test");
        return;
    }
}

/// Test MinIO connectivity.
#[tokio::test]
async fn test_minio_connectivity() {
    use std::net::TcpStream;

    let addr: std::net::SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let result = TcpStream::connect_timeout(&addr, Duration::from_secs(2));

    if result.is_ok() {
        println!("MinIO is reachable at 127.0.0.1:9000");
    } else {
        println!("MinIO not available, skipping test");
        return;
    }
}
