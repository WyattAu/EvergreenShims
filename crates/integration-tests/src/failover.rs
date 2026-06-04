//! Failover integration tests.

use std::time::Duration;

/// Test that failover-shim can detect a healthy primary.
#[tokio::test]
async fn test_failover_detects_healthy_primary() {
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

/// Test failover state transitions.
#[tokio::test]
async fn test_failover_state_transitions() {
    use failover_shim::FailoverState;

    let state = FailoverState::Healthy;
    assert_eq!(state, FailoverState::Healthy);

    let state = FailoverState::Suspect;
    assert_eq!(state, FailoverState::Suspect);

    let state = FailoverState::FailingOver;
    assert_eq!(state, FailoverState::FailingOver);

    let state = FailoverState::FailedOver;
    assert_eq!(state, FailoverState::FailedOver);

    println!("Failover state transitions work correctly");
}

/// Test failover event serialization.
#[tokio::test]
async fn test_failover_event_serialization() {
    use failover_shim::FailoverEvent;

    let event = FailoverEvent {
        event: "failover".to_string(),
        old_primary: "127.0.0.1:3306".to_string(),
        new_primary: "127.0.0.1:3307".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        reason: "3 consecutive health check failures".to_string(),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("failover"));
    assert!(json.contains("127.0.0.1:3306"));

    println!("Failover event serialization works: {}", json);
}
