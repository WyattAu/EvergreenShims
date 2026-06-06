//! Chaos shim — fault injection for resilience testing.
//!
//! Injects faults (latency, errors, partitions, process kill, disk fill, CPU stress)
//! to test application resilience against real infrastructure failures.
//!
//! ## Environment Variables
//!
//! ```text
//! CHAOS_ENABLED          Enable chaos (default: false)
//! CHAOS_LATENCY_MS       Add latency to requests (default: 0)
//! CHAOS_ERROR_RATE       Error rate 0.0-1.0 (default: 0.0)
//! CHAOS_PARTITION        Simulate network partition (default: false)
//! CHAOS_KILL_PROBABILITY Probability of killing process (default: 0.0)
//! CHAOS_DURATION_SECS    How long chaos lasts (default: 60)
//! CHAOS_TARGET           Target to apply chaos to (default: all)
//! CHAOS_BLAST_RADIUS     Blast radius as percentage 0.0-1.0 (default: 1.0)
//! CHAOS_REAL_FAULTS      Enable real fault injection via system commands (default: false)
//! CHAOS_IPTABLES_CHAIN   iptables chain for network partition (default: INPUT)
//! CHAOS_DISK_FILL_PATH   Path for disk fill fault (default: /tmp/chaos-fill)
//! CHAOS_DISK_FILL_SIZE   Size in MB for disk fill (default: 100)
//! ```

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FaultType {
    Latency,
    Error,
    Partition,
    Kill,
    PacketLoss,
}

impl std::fmt::Display for FaultType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Latency => write!(f, "latency"),
            Self::Error => write!(f, "error"),
            Self::Partition => write!(f, "partition"),
            Self::Kill => write!(f, "kill"),
            Self::PacketLoss => write!(f, "packet_loss"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChaosExperiment {
    pub id: String,
    pub name: String,
    pub fault_type: FaultType,
    pub enabled: bool,
    pub target: String,
    pub blast_radius: f64,
    pub duration_secs: u64,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub faults_injected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionResult {
    pub injected: bool,
    pub fault_type: FaultType,
    pub reason: Option<String>,
    pub delay_ms: u64,
}

pub struct ChaosShim {
    enabled: bool,
    latency_ms: u64,
    error_rate: f64,
    partition: bool,
    kill_probability: f64,
    target: String,
    blast_radius: f64,
    faults_injected: u64,
    faults_suppressed: u64,
    experiments: HashMap<String, ChaosExperiment>,
    experiment_counter: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ChaosShim {
    pub fn new() -> Self {
        Self {
            enabled: std::env::var("CHAOS_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            latency_ms: std::env::var("CHAOS_LATENCY_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            error_rate: std::env::var("CHAOS_ERROR_RATE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            partition: std::env::var("CHAOS_PARTITION")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            kill_probability: std::env::var("CHAOS_KILL_PROBABILITY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            target: std::env::var("CHAOS_TARGET").unwrap_or_else(|_| "all".to_string()),
            blast_radius: std::env::var("CHAOS_BLAST_RADIUS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
            faults_injected: 0,
            faults_suppressed: 0,
            experiments: HashMap::new(),
            experiment_counter: 0,
            shutdown_tx: None,
        }
    }

    pub fn start_experiment(
        &mut self,
        name: &str,
        fault_type: FaultType,
        target: &str,
        blast_radius: f64,
        duration_secs: u64,
    ) -> &ChaosExperiment {
        self.experiment_counter += 1;
        let experiment = ChaosExperiment {
            id: format!("exp-{:03}", self.experiment_counter),
            name: name.to_string(),
            fault_type,
            enabled: true,
            target: target.to_string(),
            blast_radius: blast_radius.clamp(0.0, 1.0),
            duration_secs,
            started_at: Some(Utc::now().to_rfc3339()),
            ended_at: None,
            faults_injected: 0,
        };
        self.experiments.insert(experiment.id.clone(), experiment);
        self.experiments
            .get(&format!("exp-{:03}", self.experiment_counter))
            .unwrap()
    }

    pub fn stop_experiment(&mut self, id: &str) -> bool {
        if let Some(exp) = self.experiments.get_mut(id) {
            exp.enabled = false;
            exp.ended_at = Some(Utc::now().to_rfc3339());
            true
        } else {
            false
        }
    }

    pub fn evaluate(&mut self, request_target: &str) -> InjectionResult {
        if !self.enabled {
            self.faults_suppressed += 1;
            return InjectionResult {
                injected: false,
                fault_type: FaultType::Latency,
                reason: Some("Chaos disabled globally".to_string()),
                delay_ms: 0,
            };
        }

        // Auto-stop expired experiments
        self.stop_expired_experiments();

        if !self.target_matches(request_target) {
            self.faults_suppressed += 1;
            return InjectionResult {
                injected: false,
                fault_type: FaultType::Latency,
                reason: Some("Target mismatch".to_string()),
                delay_ms: 0,
            };
        }

        if self.blast_radius < 1.0 {
            let roll = fastrand::f64();
            if roll > self.blast_radius {
                self.faults_suppressed += 1;
                return InjectionResult {
                    injected: false,
                    fault_type: FaultType::Latency,
                    reason: Some("Blast radius exclusion".to_string()),
                    delay_ms: 0,
                };
            }
        }

        let fault_type = self.active_fault_type();
        let delay = if fault_type == FaultType::Latency {
            self.latency_ms
        } else {
            0
        };

        if fault_type == FaultType::Error {
            let roll = fastrand::f64();
            if roll > self.error_rate && self.error_rate < 1.0 {
                self.faults_suppressed += 1;
                return InjectionResult {
                    injected: false,
                    fault_type: FaultType::Error,
                    reason: Some("Below error rate threshold".to_string()),
                    delay_ms: 0,
                };
            }
        }

        self.faults_injected += 1;

        InjectionResult {
            injected: true,
            fault_type,
            reason: None,
            delay_ms: delay,
        }
    }

    fn stop_expired_experiments(&mut self) {
        let now = Utc::now();
        for exp in self.experiments.values_mut() {
            if exp.enabled {
                if let Some(ref started_str) = exp.started_at {
                    if let Ok(started) = DateTime::parse_from_rfc3339(started_str) {
                        let elapsed = now.signed_duration_since(started.with_timezone(&Utc));
                        if elapsed >= Duration::seconds(exp.duration_secs as i64) {
                            exp.enabled = false;
                            exp.ended_at = Some(now.to_rfc3339());
                        }
                    }
                }
            }
        }
    }

    fn target_matches(&self, request_target: &str) -> bool {
        self.target == "all" || self.target.eq_ignore_ascii_case(request_target)
    }

    fn active_fault_type(&self) -> FaultType {
        if self.partition {
            FaultType::Partition
        } else if self.latency_ms > 0 {
            FaultType::Latency
        } else if self.error_rate > 0.0 {
            FaultType::Error
        } else if self.kill_probability > 0.0 {
            FaultType::Kill
        } else {
            FaultType::Latency
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_error_rate(&mut self, rate: f64) {
        self.error_rate = rate.clamp(0.0, 1.0);
    }

    pub fn set_latency(&mut self, ms: u64) {
        self.latency_ms = ms;
    }

    pub fn get_experiment(&self, id: &str) -> Option<&ChaosExperiment> {
        self.experiments.get(id)
    }

    pub fn active_experiments(&self) -> Vec<&ChaosExperiment> {
        self.experiments.values().filter(|e| e.enabled).collect()
    }

    pub fn injection_rate(&self) -> f64 {
        let total = self.faults_injected + self.faults_suppressed;
        if total == 0 {
            0.0
        } else {
            self.faults_injected as f64 / total as f64
        }
    }

    pub fn is_active(&self) -> bool {
        self.enabled && !self.active_experiments().is_empty()
    }

    /// Apply artificial latency based on the injection result.
    pub async fn apply_latency(result: &InjectionResult) {
        if result.injected && result.delay_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(result.delay_ms)).await;
        }
    }

    /// Inject an error based on the injection result, returning an error if applicable.
    pub fn inject_error(result: &InjectionResult) -> anyhow::Result<()> {
        if result.injected && result.fault_type == FaultType::Error {
            anyhow::bail!("Chaos-injected error");
        }
        Ok(())
    }

    // =========================================================================
    // Real Fault Injection (requires elevated permissions)
    // =========================================================================

    /// Create a network partition by blocking traffic to/from a target IP using iptables.
    ///
    /// Requires root/sudo. Creates a rule in the specified chain (default: INPUT).
    /// Returns a rule identifier that can be used with `remove_network_partition`.
    #[allow(dead_code)]
    pub async fn create_network_partition(target_ip: &str, chain: &str) -> anyhow::Result<String> {
        let rule_id = format!("chaos-{}", uuid_short());

        tracing::warn!(
            "Creating network partition for {} (chain: {}, rule: {})",
            target_ip,
            chain,
            rule_id
        );

        let output = tokio::process::Command::new("sudo")
            .args([
                "iptables",
                "-A",
                chain,
                "-s",
                target_ip,
                "-j",
                "DROP",
                "-m",
                "comment",
                "--comment",
                &rule_id,
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create network partition: {}", stderr);
        }

        tracing::info!(
            "Network partition created for {} (rule: {})",
            target_ip,
            rule_id
        );
        Ok(rule_id)
    }

    /// Remove a network partition by deleting the iptables rule.
    #[allow(dead_code)]
    pub async fn remove_network_partition(rule_id: &str, chain: &str) -> anyhow::Result<()> {
        tracing::info!("Removing network partition (rule: {})", rule_id);

        let output = tokio::process::Command::new("sudo")
            .args([
                "iptables",
                "-D",
                chain,
                "-m",
                "comment",
                "--comment",
                rule_id,
                "-j",
                "DROP",
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to remove network partition: {}", stderr);
        }

        tracing::info!("Network partition removed (rule: {})", rule_id);
        Ok(())
    }

    /// Kill a process by PID with SIGKILL.
    ///
    /// Used for process crash fault injection.
    #[allow(dead_code)]
    pub async fn kill_process(pid: u32) -> anyhow::Result<()> {
        tracing::warn!("Killing process PID {} (SIGKILL)", pid);

        let output = tokio::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to kill process {}: {}", pid, stderr);
        }

        tracing::info!("Process {} killed", pid);
        Ok(())
    }

    /// Fill disk space at the specified path.
    ///
    /// Creates a large file to simulate disk-full conditions.
    /// Use `remove_disk_fill` to clean up afterward.
    #[allow(dead_code)]
    pub async fn create_disk_fill(path: &str, size_mb: u64) -> anyhow::Result<()> {
        tracing::warn!("Creating disk fill at {} ({} MB)", path, size_mb);

        let output = tokio::process::Command::new("fallocate")
            .args(["-l", &format!("{}M", size_mb), path])
            .output()
            .await?;

        if !output.status.success() {
            // Fall back to dd if fallocate is not available
            let output = tokio::process::Command::new("dd")
                .args([
                    "if=/dev/zero",
                    &format!("of={}", path),
                    "bs=1M",
                    &format!("count={}", size_mb),
                ])
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("Failed to create disk fill: {}", stderr);
            }
        }

        tracing::info!("Disk fill created: {} ({} MB)", path, size_mb);
        Ok(())
    }

    /// Remove a disk fill file.
    #[allow(dead_code)]
    pub async fn remove_disk_fill(path: &str) -> anyhow::Result<()> {
        tracing::info!("Removing disk fill: {}", path);
        tokio::fs::remove_file(path).await?;
        tracing::info!("Disk fill removed: {}", path);
        Ok(())
    }

    /// Apply CPU stress using stress-ng.
    ///
    /// Runs CPU stress for the specified duration.
    #[allow(dead_code)]
    pub async fn apply_cpu_stress(cores: u32, duration_secs: u64) -> anyhow::Result<()> {
        tracing::warn!("Applying CPU stress ({} cores, {}s)", cores, duration_secs);

        let output = tokio::process::Command::new("stress-ng")
            .args([
                "--cpu",
                &cores.to_string(),
                "--timeout",
                &format!("{}s", duration_secs),
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("CPU stress failed: {}", stderr);
        }

        tracing::info!("CPU stress completed ({} cores, {}s)", cores, duration_secs);
        Ok(())
    }
}

/// Generate a short UUID-like identifier for rule tracking.
fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}", t as u64)
}

impl Default for ChaosShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ChaosShim {
    fn name(&self) -> &str {
        "chaos"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "ChaosShim initialized (enabled={}, latency={}ms, error_rate={})",
            self.enabled,
            self.latency_ms,
            self.error_rate
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ChaosShim started (enabled={})", self.enabled);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        for exp in self.experiments.values_mut() {
            if exp.enabled {
                exp.enabled = false;
                exp.ended_at = Some(Utc::now().to_rfc3339());
            }
        }
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("ChaosShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let active = self.active_experiments().len();
        vec![
            Metric::new("chaos_enabled", if self.enabled { 1.0 } else { 0.0 }),
            Metric::new("chaos_faults_injected", self.faults_injected as f64),
            Metric::new("chaos_faults_suppressed", self.faults_suppressed as f64),
            Metric::new("chaos_active_experiments", active as f64),
            Metric::new("chaos_injection_rate", self.injection_rate()),
            Metric::new("chaos_error_rate", self.error_rate),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fault_type_display() {
        assert_eq!(FaultType::Latency.to_string(), "latency");
        assert_eq!(FaultType::Error.to_string(), "error");
        assert_eq!(FaultType::Partition.to_string(), "partition");
    }

    #[test]
    fn test_new_defaults() {
        let shim = ChaosShim::new();
        assert!(!shim.enabled);
        assert_eq!(shim.latency_ms, 0);
        assert_eq!(shim.error_rate, 0.0);
        assert_eq!(shim.blast_radius, 1.0);
    }

    #[test]
    fn test_evaluate_disabled() {
        let mut shim = ChaosShim::new();
        let result = shim.evaluate("service-a");
        assert!(!result.injected);
        assert_eq!(shim.faults_suppressed, 1);
    }

    #[test]
    fn test_evaluate_latency() {
        let mut shim = ChaosShim {
            enabled: true,
            latency_ms: 500,
            target: "all".to_string(),
            blast_radius: 1.0,
            ..ChaosShim::new()
        };
        let result = shim.evaluate("service-a");
        assert!(result.injected);
        assert_eq!(result.fault_type, FaultType::Latency);
        assert_eq!(result.delay_ms, 500);
        assert_eq!(shim.faults_injected, 1);
    }

    #[test]
    fn test_evaluate_partition() {
        let mut shim = ChaosShim {
            enabled: true,
            partition: true,
            target: "all".to_string(),
            blast_radius: 1.0,
            ..ChaosShim::new()
        };
        let result = shim.evaluate("service-a");
        assert!(result.injected);
        assert_eq!(result.fault_type, FaultType::Partition);
    }

    #[test]
    fn test_evaluate_target_mismatch() {
        let mut shim = ChaosShim {
            enabled: true,
            latency_ms: 100,
            target: "service-b".to_string(),
            blast_radius: 1.0,
            ..ChaosShim::new()
        };
        let result = shim.evaluate("service-a");
        assert!(!result.injected);
    }

    #[test]
    fn test_start_and_stop_experiment() {
        let mut shim = ChaosShim::new();
        let exp = shim.start_experiment("test-lag", FaultType::Latency, "service-a", 0.5, 60);
        assert!(exp.enabled);

        let id = exp.id.clone();
        assert!(shim.stop_experiment(&id));
        assert!(!shim.get_experiment(&id).unwrap().enabled);
    }

    #[test]
    fn test_stop_nonexistent_experiment() {
        let mut shim = ChaosShim::new();
        assert!(!shim.stop_experiment("nonexistent"));
    }

    #[test]
    fn test_active_experiments() {
        let mut shim = ChaosShim::new();
        shim.start_experiment("exp1", FaultType::Latency, "all", 1.0, 60);
        let exp2_id = {
            let exp2 = shim.start_experiment("exp2", FaultType::Error, "all", 1.0, 60);
            exp2.id.clone()
        };
        shim.stop_experiment(&exp2_id);

        assert_eq!(shim.active_experiments().len(), 1);
    }

    #[test]
    fn test_set_error_rate_clamped() {
        let mut shim = ChaosShim::new();
        shim.set_error_rate(1.5);
        assert_eq!(shim.error_rate, 1.0);

        shim.set_error_rate(-0.5);
        assert_eq!(shim.error_rate, 0.0);

        shim.set_error_rate(0.75);
        assert_eq!(shim.error_rate, 0.75);
    }

    #[test]
    fn test_injection_rate() {
        let shim = ChaosShim {
            faults_injected: 8,
            faults_suppressed: 2,
            ..ChaosShim::new()
        };
        assert!((shim.injection_rate() - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_injection_rate_zero() {
        let shim = ChaosShim::new();
        assert_eq!(shim.injection_rate(), 0.0);
    }

    #[test]
    fn test_is_active() {
        let mut shim = ChaosShim {
            enabled: true,
            ..ChaosShim::new()
        };
        shim.start_experiment("exp", FaultType::Latency, "all", 1.0, 60);
        assert!(shim.is_active());

        shim.set_enabled(false);
        assert!(!shim.is_active());
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = ChaosShim {
            enabled: true,
            faults_injected: 10,
            faults_suppressed: 3,
            error_rate: 0.5,
            ..ChaosShim::new()
        };
        shim.start_experiment("exp", FaultType::Latency, "all", 1.0, 60);

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 6);
        assert_eq!(metrics[0].value, 1.0);
        assert_eq!(metrics[1].value, 10.0);
        assert_eq!(metrics[3].value, 1.0);
        assert_eq!(metrics[5].value, 0.5);
    }

    #[test]
    fn test_experiment_expiry() {
        let mut shim = ChaosShim {
            enabled: true,
            ..ChaosShim::new()
        };
        // Start experiment with 0-second duration (already expired)
        let exp = shim.start_experiment("expired", FaultType::Latency, "all", 1.0, 0);
        let id = exp.id.clone();

        // First evaluate should auto-stop the expired experiment
        shim.evaluate("all");
        assert!(!shim.get_experiment(&id).unwrap().enabled);
        assert!(shim.get_experiment(&id).unwrap().ended_at.is_some());
    }

    #[test]
    fn test_blast_radius_with_random() {
        let mut shim = ChaosShim {
            enabled: true,
            latency_ms: 100,
            target: "all".to_string(),
            blast_radius: 0.0, // No requests should be injected
            ..ChaosShim::new()
        };
        let result = shim.evaluate("service-a");
        assert!(!result.injected);
        assert_eq!(result.reason.as_deref(), Some("Blast radius exclusion"));
    }

    #[tokio::test]
    async fn test_apply_latency() {
        let result = InjectionResult {
            injected: true,
            fault_type: FaultType::Latency,
            reason: None,
            delay_ms: 10,
        };
        let start = tokio::time::Instant::now();
        ChaosShim::apply_latency(&result).await;
        assert!(start.elapsed() >= tokio::time::Duration::from_millis(10));
    }

    #[test]
    fn test_inject_error() {
        let result = InjectionResult {
            injected: true,
            fault_type: FaultType::Error,
            reason: None,
            delay_ms: 0,
        };
        assert!(ChaosShim::inject_error(&result).is_err());

        let no_error = InjectionResult {
            injected: false,
            fault_type: FaultType::Error,
            reason: None,
            delay_ms: 0,
        };
        assert!(ChaosShim::inject_error(&no_error).is_ok());
    }

    #[test]
    fn test_is_active_no_experiments() {
        let shim = ChaosShim {
            enabled: true,
            ..ChaosShim::new()
        };
        assert!(!shim.is_active());
    }
}
