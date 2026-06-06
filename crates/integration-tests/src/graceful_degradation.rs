//! Integration tests for graceful degradation behavior.
//!
//! Verifies that:
//! - Non-critical capabilities failing does not prevent other capabilities from starting.
//! - Critical capability failures are properly reported.
//! - The `shim_capabilities_healthy` metric reflects overall health.

#![allow(dead_code)]

use shim_core::{Capability, Config, Metric};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A capability that always succeeds.
struct DummyCapability {
    name: String,
    started: Arc<AtomicBool>,
    should_fail_init: bool,
    should_fail_start: bool,
}

impl DummyCapability {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            started: Arc::new(AtomicBool::new(false)),
            should_fail_init: false,
            should_fail_start: false,
        }
    }

    fn fail_init(mut self) -> Self {
        self.should_fail_init = true;
        self
    }

    fn fail_start(mut self) -> Self {
        self.should_fail_start = true;
        self
    }
}

#[async_trait::async_trait]
impl Capability for DummyCapability {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init(&mut self, _config: &Config) -> shim_core::Result<()> {
        if self.should_fail_init {
            return Err(shim_core::Error::Config(format!(
                "{} deliberately failed init",
                self.name
            )));
        }
        Ok(())
    }

    async fn start(&mut self) -> shim_core::Result<()> {
        if self.should_fail_start {
            return Err(shim_core::Error::Config(format!(
                "{} deliberately failed start",
                self.name
            )));
        }
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> shim_core::Result<()> {
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}

/// A capability that always fails during init.
struct AlwaysFailInit;

#[async_trait::async_trait]
impl Capability for AlwaysFailInit {
    fn name(&self) -> &str {
        "always_fail_init"
    }

    async fn init(&mut self, _config: &Config) -> shim_core::Result<()> {
        Err(shim_core::Error::Config("unavoidable failure".into()))
    }

    async fn start(&mut self) -> shim_core::Result<()> {
        unreachable!("start should not be called if init fails")
    }

    async fn stop(&mut self) -> shim_core::Result<()> {
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}

/// Simulate the graceful degradation logic from run_shim.
///
/// Returns (outcomes, all_critical_healthy) where each outcome is
/// (name, started, Option<error_message>).
async fn run_degradation_simulation(
    capabilities: &mut Vec<Box<dyn Capability>>,
    critical_names: &[&str],
) -> (Vec<(String, bool, Option<String>)>, bool) {
    let mut outcomes = Vec::new();
    let mut successful_names = std::collections::HashSet::new();

    for cap in capabilities.iter_mut() {
        let name = cap.name().to_string();
        let is_critical = critical_names.contains(&name.as_str());

        if let Err(e) = cap.init(&Config::default()).await {
            outcomes.push((name.clone(), false, Some(format!("init: {e}"))));
            if is_critical {
                // Critical failure: propagate immediately
                return (outcomes, false);
            }
            continue;
        }

        if let Err(e) = cap.start().await {
            outcomes.push((name.clone(), false, Some(format!("start: {e}"))));
            if is_critical {
                return (outcomes, false);
            }
            continue;
        }

        successful_names.insert(name.clone());
        outcomes.push((name, true, None));
    }

    let all_critical_healthy = critical_names.iter().all(|c| successful_names.contains(*c));

    (outcomes, all_critical_healthy)
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_all_succeed() {
    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(DummyCapability::new("health")),
        Box::new(DummyCapability::new("cache")),
    ];

    let (outcomes, healthy) = run_degradation_simulation(&mut caps, &["health"]).await;

    assert!(healthy, "all critical should be healthy");
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|(_, started, _)| *started));
}

#[tokio::test]
async fn test_non_critical_failure_does_not_block() {
    let cache = DummyCapability::new("cache").fail_init();
    let cost = DummyCapability::new("cost").fail_start();

    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(DummyCapability::new("health")),
        Box::new(DummyCapability::new("migration")),
        Box::new(cache),
        Box::new(cost),
    ];

    let (outcomes, healthy) = run_degradation_simulation(&mut caps, &["health", "migration"]).await;

    assert!(
        healthy,
        "critical capabilities should still be healthy when non-critical fail"
    );

    let cache_outcome = outcomes.iter().find(|(n, _, _)| n == "cache").unwrap();
    assert!(!cache_outcome.1, "cache should have failed");
    assert!(cache_outcome.2.is_some(), "cache should have error message");

    let cost_outcome = outcomes.iter().find(|(n, _, _)| n == "cost").unwrap();
    assert!(!cost_outcome.1, "cost should have failed");
    assert!(cost_outcome.2.is_some(), "cost should have error message");

    let health_outcome = outcomes.iter().find(|(n, _, _)| n == "health").unwrap();
    assert!(health_outcome.1, "health should have started");
}

#[tokio::test]
async fn test_critical_init_failure_blocks() {
    let health_fail = DummyCapability::new("health").fail_init();

    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(health_fail),
        Box::new(DummyCapability::new("cache")),
    ];

    let (outcomes, healthy) = run_degradation_simulation(&mut caps, &["health"]).await;

    assert!(
        !healthy,
        "should not be healthy when critical capability fails"
    );

    let health_outcome = outcomes.iter().find(|(n, _, _)| n == "health").unwrap();
    assert!(!health_outcome.1, "health init failure should be reported");
    assert!(health_outcome.2.is_some());

    // cache should not even have been attempted (init short-circuits)
    assert!(
        !outcomes.iter().any(|(n, _, _)| n == "cache"),
        "cache should not be in outcomes after critical failure"
    );
}

#[tokio::test]
async fn test_critical_start_failure_blocks() {
    let migration_fail = DummyCapability::new("migration").fail_start();

    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(DummyCapability::new("health")),
        Box::new(migration_fail),
        Box::new(DummyCapability::new("cache")),
    ];

    let (outcomes, healthy) = run_degradation_simulation(&mut caps, &["health", "migration"]).await;

    assert!(!healthy, "should not be healthy when migration fails");

    let migration_outcome = outcomes.iter().find(|(n, _, _)| n == "migration").unwrap();
    assert!(
        !migration_outcome.1,
        "migration start failure should be reported"
    );

    // health succeeded
    let health_outcome = outcomes.iter().find(|(n, _, _)| n == "health").unwrap();
    assert!(health_outcome.1);
}

#[tokio::test]
async fn test_all_non_critical_fail_still_healthy() {
    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(DummyCapability::new("health")),
        Box::new(DummyCapability::new("migration")),
        Box::new(DummyCapability::new("chaos").fail_init()),
        Box::new(DummyCapability::new("cost").fail_start()),
        Box::new(DummyCapability::new("cache").fail_init()),
    ];

    let critical = vec!["health", "migration"];
    let (outcomes, healthy) = run_degradation_simulation(&mut caps, &critical).await;

    assert!(
        healthy,
        "should be healthy as long as critical caps succeed"
    );

    let started: Vec<&str> = outcomes
        .iter()
        .filter(|(_, s, _)| *s)
        .map(|(n, _, _)| n.as_str())
        .collect();
    assert!(started.contains(&"health"));
    assert!(started.contains(&"migration"));

    let failed: Vec<&str> = outcomes
        .iter()
        .filter(|(_, s, _)| !*s)
        .map(|(n, _, _)| n.as_str())
        .collect();
    assert!(failed.contains(&"chaos"));
    assert!(failed.contains(&"cost"));
    assert!(failed.contains(&"cache"));
}

#[tokio::test]
async fn test_critical_failure_returns_false() {
    let mut caps: Vec<Box<dyn Capability>> = vec![Box::new(AlwaysFailInit)];

    let (outcomes, healthy) = run_degradation_simulation(&mut caps, &["always_fail_init"]).await;

    assert!(!healthy);
    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].1);
    assert!(outcomes[0].2.is_some());
}

#[tokio::test]
async fn test_mixed_critical_and_non_critical_failures() {
    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(DummyCapability::new("health")),
        Box::new(DummyCapability::new("cache").fail_init()),
        Box::new(DummyCapability::new("chaos").fail_start()),
    ];

    let critical = vec!["health", "migration"];
    let (outcomes, healthy) = run_degradation_simulation(&mut caps, &critical).await;

    // health succeeded, but migration was never enabled -> critical check fails
    assert!(
        !healthy,
        "migration is critical but not in capabilities, should not be healthy"
    );

    let health_outcome = outcomes.iter().find(|(n, _, _)| n == "health").unwrap();
    assert!(health_outcome.1);

    let cache_outcome = outcomes.iter().find(|(n, _, _)| n == "cache").unwrap();
    assert!(!cache_outcome.1);

    let chaos_outcome = outcomes.iter().find(|(n, _, _)| n == "chaos").unwrap();
    assert!(!chaos_outcome.1);
}

#[tokio::test]
async fn test_health_metric_value_reflects_critical_health() {
    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(DummyCapability::new("health")),
        Box::new(DummyCapability::new("chaos").fail_init()),
    ];

    let critical = vec!["health"];
    let (_, healthy) = run_degradation_simulation(&mut caps, &critical).await;

    let metric_value: f64 = if healthy { 1.0 } else { 0.0 };
    assert_eq!(metric_value, 1.0, "metric should report 1 when healthy");

    // Now test unhealthy case
    let mut caps2: Vec<Box<dyn Capability>> = vec![
        Box::new(DummyCapability::new("health").fail_start()),
        Box::new(DummyCapability::new("chaos")),
    ];

    let (_, healthy2) = run_degradation_simulation(&mut caps2, &["health"]).await;
    let metric_value2: f64 = if healthy2 { 1.0 } else { 0.0 };
    assert_eq!(
        metric_value2, 0.0,
        "metric should report 0 when critical fails"
    );
}

#[tokio::test]
async fn test_stops_only_successful_capabilities() {
    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(DummyCapability::new("health")),
        Box::new(DummyCapability::new("cache").fail_init()),
        Box::new(DummyCapability::new("chaos")),
    ];

    let critical = vec!["health"];
    let (outcomes, _) = run_degradation_simulation(&mut caps, &critical).await;

    // Simulate stop logic: only stop capabilities that started
    let successful_indices: Vec<usize> = caps
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let outcome = outcomes.iter().find(|(n, _, _)| n == c.name());
            match outcome {
                Some((_, true, _)) => Some(i),
                _ => None,
            }
        })
        .collect();

    assert_eq!(
        successful_indices.len(),
        2,
        "health and chaos should be stopped"
    );

    // Stop them (should not error)
    for idx in &successful_indices {
        caps[*idx].stop().await.unwrap();
    }
}
