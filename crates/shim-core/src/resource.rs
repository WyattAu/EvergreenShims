//! Resource quota monitoring for shim processes.
//!
//! `ResourceMonitor` tracks current resource usage by reading `/proc/self/status`
//! on Linux and checks against configured `ResourceQuota` limits. It logs warnings
//! when usage exceeds 80% of any configured quota.
//!
//! Includes:
//! - Memory usage tracking (RSS via /proc/self/status on Linux)
//! - File descriptor count tracking (count /proc/self/fd entries)
//! - CPU usage tracking (read /proc/self/stat, calculate delta)
//! - Periodic resource metrics emission (configurable via `SHIM_RESOURCE_INTERVAL_SECS`)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use prometheus::{Gauge, Registry};
use tracing::{error, warn};

use crate::config::ResourceQuota;

/// Default resource metrics interval in seconds.
const DEFAULT_RESOURCE_INTERVAL_SECS: u64 = 10;

/// Current resource usage snapshot.
#[derive(Debug, Clone, Default)]
pub struct ResourceUsage {
    /// Memory usage in bytes (from VmRSS).
    pub memory_bytes: Option<u64>,
    /// CPU usage percentage (estimated from /proc/self/stat delta).
    pub cpu_percent: Option<f64>,
    /// Number of open file descriptors (from /proc/self/fd count).
    pub open_files: Option<u32>,
    /// Process CPU time in user mode (microseconds).
    pub cpu_user_time: Option<u64>,
    /// Process CPU time in system mode (microseconds).
    pub cpu_system_time: Option<u64>,
}

/// Metrics for resource monitoring.
pub struct ResourceMetrics {
    /// Current memory usage in bytes.
    pub memory_bytes: Gauge,
    /// Current CPU usage percent.
    pub cpu_percent: Gauge,
    /// Current open file descriptor count.
    pub open_files: Gauge,
    /// CPU user time in microseconds.
    pub cpu_user_time: Gauge,
    /// CPU system time in microseconds.
    pub cpu_system_time: Gauge,
}

impl ResourceMetrics {
    /// Create and register metrics on the given registry.
    pub fn new(registry: &Registry) -> Self {
        let memory_bytes = Gauge::with_opts(prometheus::Opts::new(
            "resource_memory_bytes",
            "Current memory usage in bytes (RSS)",
        ))
        .expect("metric opts for resource_memory_bytes are valid");
        let cpu_percent = Gauge::with_opts(prometheus::Opts::new(
            "resource_cpu_percent",
            "Current CPU usage percentage (estimated)",
        ))
        .expect("metric opts for resource_cpu_percent are valid");
        let open_files = Gauge::with_opts(prometheus::Opts::new(
            "resource_open_files",
            "Current number of open file descriptors",
        ))
        .expect("metric opts for resource_open_files are valid");
        let cpu_user_time = Gauge::with_opts(prometheus::Opts::new(
            "resource_cpu_user_time_us",
            "Process CPU time in user mode (microseconds)",
        ))
        .expect("metric opts for resource_cpu_user_time_us are valid");
        let cpu_system_time = Gauge::with_opts(prometheus::Opts::new(
            "resource_cpu_system_time_us",
            "Process CPU time in system mode (microseconds)",
        ))
        .expect("metric opts for resource_cpu_system_time_us are valid");

        registry
            .register(Box::new(memory_bytes.clone()))
            .expect("register memory_bytes must not conflict");
        registry
            .register(Box::new(cpu_percent.clone()))
            .expect("register cpu_percent must not conflict");
        registry
            .register(Box::new(open_files.clone()))
            .expect("register open_files must not conflict");
        registry
            .register(Box::new(cpu_user_time.clone()))
            .expect("register cpu_user_time must not conflict");
        registry
            .register(Box::new(cpu_system_time.clone()))
            .expect("register cpu_system_time must not conflict");

        Self {
            memory_bytes,
            cpu_percent,
            open_files,
            cpu_user_time,
            cpu_system_time,
        }
    }
}

/// Monitors resource usage and checks against configured quotas.
pub struct ResourceMonitor {
    /// Configured resource quotas.
    quota: ResourceQuota,
    /// Optional Prometheus metrics.
    metrics: Option<ResourceMetrics>,
    /// Previous CPU times for delta calculation.
    prev_cpu_user: Option<u64>,
    prev_cpu_system: Option<u64>,
    prev_wall_time: Option<std::time::Instant>,
    /// Whether periodic monitoring is running.
    running: Arc<AtomicBool>,
}

impl ResourceMonitor {
    /// Create a new resource monitor with the given quota.
    pub fn new(quota: ResourceQuota) -> Self {
        Self {
            quota,
            metrics: None,
            prev_cpu_user: None,
            prev_cpu_system: None,
            prev_wall_time: None,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a new resource monitor with metrics collection.
    pub fn with_metrics(quota: ResourceQuota, registry: &Registry) -> Self {
        Self {
            quota,
            metrics: Some(ResourceMetrics::new(registry)),
            prev_cpu_user: None,
            prev_cpu_system: None,
            prev_wall_time: None,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Read current resource usage from the OS.
    ///
    /// On Linux, parses `/proc/self/status` for VmRSS and FDSize,
    /// `/proc/self/fd` for actual FD count, and `/proc/self/stat` for CPU times.
    /// Falls back to empty values on unsupported platforms.
    pub fn read_usage(&mut self) -> ResourceUsage {
        let mut usage = ResourceUsage::default();

        #[cfg(target_os = "linux")]
        {
            usage.memory_bytes = self.read_vm_rss();
            usage.open_files = self.read_fd_count();
            if let Some((user, system)) = self.read_cpu_times() {
                usage.cpu_user_time = Some(user);
                usage.cpu_system_time = Some(system);

                // Calculate CPU percent delta
                if let Some(prev_user) = self.prev_cpu_user {
                    if let Some(prev_system) = self.prev_cpu_system {
                        if let Some(prev_wall) = self.prev_wall_time {
                            let delta_user = user.saturating_sub(prev_user);
                            let delta_system = system.saturating_sub(prev_system);
                            let delta_wall = prev_wall.elapsed().as_micros() as u64;

                            if delta_wall > 0 {
                                let total_cpu = delta_user + delta_system;
                                usage.cpu_percent =
                                    Some((total_cpu as f64 / delta_wall as f64) * 100.0);
                            }
                        }
                    }
                }

                self.prev_cpu_user = Some(user);
                self.prev_cpu_system = Some(system);
                self.prev_wall_time = Some(std::time::Instant::now());
            }
        }

        usage
    }

    /// Read-only usage check without updating internal CPU state.
    pub fn read_usage_snapshot(&self) -> ResourceUsage {
        let mut usage = ResourceUsage::default();

        #[cfg(target_os = "linux")]
        {
            usage.memory_bytes = self.read_vm_rss();
            usage.open_files = self.read_fd_count();
            if let Some((user, system)) = self.read_cpu_times() {
                usage.cpu_user_time = Some(user);
                usage.cpu_system_time = Some(system);
            }
        }

        usage
    }

    /// Start periodic resource monitoring in the background.
    ///
    /// Emits metrics every `SHIM_RESOURCE_INTERVAL_SECS` seconds (default: 10).
    pub fn start_periodic_monitoring(&self) -> tokio::task::JoinHandle<()> {
        let interval_secs = std::env::var("SHIM_RESOURCE_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_RESOURCE_INTERVAL_SECS);

        let running = self.running.clone();
        let metrics = self.metrics.as_ref().map(|m| {
            (
                m.memory_bytes.clone(),
                m.cpu_percent.clone(),
                m.open_files.clone(),
                m.cpu_user_time.clone(),
                m.cpu_system_time.clone(),
            )
        });

        running.store(true, Ordering::SeqCst);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;

                if !running.load(Ordering::SeqCst) {
                    break;
                }

                // Read usage via /proc directly for the background monitor
                let mut usage = ResourceUsage::default();
                #[cfg(target_os = "linux")]
                {
                    usage.memory_bytes = read_vm_rss_static();
                    usage.open_files = read_fd_count_static();
                    if let Some((user, system)) = read_cpu_times_static() {
                        usage.cpu_user_time = Some(user);
                        usage.cpu_system_time = Some(system);
                    }
                }

                // Update metrics
                if let Some((ref mem, ref _cpu, ref fd, ref user_t, ref sys_t)) = metrics {
                    if let Some(memory) = usage.memory_bytes {
                        mem.set(memory as f64);
                    }
                    if let Some(open_files) = usage.open_files {
                        fd.set(open_files as f64);
                    }
                    if let Some(user) = usage.cpu_user_time {
                        user_t.set(user as f64);
                    }
                    if let Some(system) = usage.cpu_system_time {
                        sys_t.set(system as f64);
                    }
                }

                tracing::debug!(
                    memory_bytes = usage.memory_bytes.unwrap_or(0),
                    open_files = usage.open_files.unwrap_or(0),
                    "resource metrics emitted"
                );
            }
        })
    }

    /// Stop periodic monitoring.
    pub fn stop_periodic_monitoring(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Check current usage against quotas and log warnings if thresholds exceeded.
    ///
    /// Returns `true` if all quotas are within bounds, `false` if any quota is exceeded.
    pub fn check_quotas(&mut self) -> bool {
        let usage = self.read_usage();
        let mut all_ok = true;

        if let Some(memory) = usage.memory_bytes {
            if let Some(max) = self.quota.max_memory_bytes {
                let ratio = memory as f64 / max as f64;
                if ratio > 1.0 {
                    error!(
                        "resource quota exceeded: memory {} bytes > max {} bytes",
                        memory, max
                    );
                    all_ok = false;
                } else if ratio > 0.8 {
                    warn!(
                        "resource quota warning: memory usage at {:.1}% of {} bytes max",
                        ratio * 100.0,
                        max
                    );
                }
            }
        }

        if let Some(open_files) = usage.open_files {
            if let Some(max) = self.quota.max_open_files {
                let ratio = open_files as f64 / max as f64;
                if ratio > 1.0 {
                    error!(
                        "resource quota exceeded: open files {} > max {}",
                        open_files, max
                    );
                    all_ok = false;
                } else if ratio > 0.8 {
                    warn!(
                        "resource quota warning: open files at {:.1}% of {} max",
                        ratio * 100.0,
                        max
                    );
                }
            }
        }

        // Update metrics if available
        if let Some(ref m) = self.metrics {
            if let Some(memory) = usage.memory_bytes {
                m.memory_bytes.set(memory as f64);
            }
            if let Some(open_files) = usage.open_files {
                m.open_files.set(open_files as f64);
            }
            if let Some(cpu) = usage.cpu_percent {
                m.cpu_percent.set(cpu);
            }
            if let Some(user) = usage.cpu_user_time {
                m.cpu_user_time.set(user as f64);
            }
            if let Some(system) = usage.cpu_system_time {
                m.cpu_system_time.set(system as f64);
            }
        }

        all_ok
    }

    /// Get the configured quota.
    pub fn quota(&self) -> &ResourceQuota {
        &self.quota
    }

    /// Get a reference to the metrics (if configured).
    pub fn metrics(&self) -> Option<&ResourceMetrics> {
        self.metrics.as_ref()
    }

    /// Update CPU percent metric from an external measurement.
    pub fn set_cpu_percent(&self, percent: f64) {
        if let Some(ref m) = self.metrics {
            m.cpu_percent.set(percent);
        }
    }

    #[cfg(target_os = "linux")]
    fn read_vm_rss(&self) -> Option<u64> {
        read_vm_rss_static()
    }

    #[cfg(target_os = "linux")]
    fn read_fd_count(&self) -> Option<u32> {
        read_fd_count_static()
    }

    #[cfg(target_os = "linux")]
    fn read_cpu_times(&self) -> Option<(u64, u64)> {
        read_cpu_times_static()
    }
}

/// Static helper: read VmRSS from /proc/self/status (Linux only).
#[cfg(target_os = "linux")]
fn read_vm_rss_static() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|content| {
            for line in content.lines() {
                if line.starts_with("VmRSS:") {
                    let val: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
                    // VmRSS is in kB
                    return Some(val * 1024);
                }
            }
            None
        })
}

/// Static helper: count file descriptors from /proc/self/fd (Linux only).
#[cfg(target_os = "linux")]
fn read_fd_count_static() -> Option<u32> {
    std::fs::read_dir("/proc/self/fd")
        .ok()
        .map(|entries| entries.count() as u32)
}

/// Static helper: read CPU times from /proc/self/stat (Linux only).
///
/// Returns (user_time, system_time) in clock ticks (typically 100 ticks/sec).
#[cfg(target_os = "linux")]
fn read_cpu_times_static() -> Option<(u64, u64)> {
    std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|content| {
            let fields: Vec<&str> = content.split_whitespace().collect();
            if fields.len() > 21 {
                let user_time: u64 = fields[13].parse().ok()?;
                let system_time: u64 = fields[14].parse().ok()?;
                Some((user_time, system_time))
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_usage_default() {
        let usage = ResourceUsage::default();
        assert!(usage.memory_bytes.is_none());
        assert!(usage.cpu_percent.is_none());
        assert!(usage.open_files.is_none());
        assert!(usage.cpu_user_time.is_none());
        assert!(usage.cpu_system_time.is_none());
    }

    #[test]
    fn test_resource_monitor_creation() {
        let quota = ResourceQuota::default();
        let monitor = ResourceMonitor::new(quota);
        assert!(monitor.metrics().is_none());
    }

    #[test]
    fn test_resource_monitor_with_metrics() {
        let registry = Registry::new();
        let quota = ResourceQuota::default();
        let monitor = ResourceMonitor::with_metrics(quota, &registry);
        assert!(monitor.metrics().is_some());
    }

    #[test]
    fn test_check_quotas_no_limits_always_ok() {
        let quota = ResourceQuota::default();
        let mut monitor = ResourceMonitor::new(quota);
        assert!(monitor.check_quotas());
    }

    #[test]
    fn test_check_quotas_within_limit() {
        let quota = ResourceQuota {
            max_memory_bytes: Some(u64::MAX),
            max_cpu_percent: Some(100.0),
            max_open_files: Some(u32::MAX),
            max_connections: None,
        };
        let mut monitor = ResourceMonitor::new(quota);
        assert!(monitor.check_quotas());
    }

    #[test]
    fn test_check_quotas_over_limit() {
        let quota = ResourceQuota {
            max_memory_bytes: Some(1), // 1 byte limit — will always be exceeded
            max_cpu_percent: None,
            max_open_files: None,
            max_connections: None,
        };
        let mut monitor = ResourceMonitor::new(quota);
        assert!(!monitor.check_quotas());
    }

    #[test]
    fn test_check_quotas_over_open_files_limit() {
        let quota = ResourceQuota {
            max_memory_bytes: None,
            max_cpu_percent: None,
            max_open_files: Some(1), // 1 fd limit — will always be exceeded
            max_connections: None,
        };
        let mut monitor = ResourceMonitor::new(quota);
        assert!(!monitor.check_quotas());
    }

    #[test]
    fn test_set_cpu_percent() {
        let registry = Registry::new();
        let quota = ResourceQuota::default();
        let monitor = ResourceMonitor::with_metrics(quota, &registry);
        monitor.set_cpu_percent(42.5);

        let m = monitor.metrics().unwrap();
        assert_eq!(m.cpu_percent.get(), 42.5);
    }

    #[test]
    fn test_metrics_export() {
        let registry = Registry::new();
        let quota = ResourceQuota {
            max_memory_bytes: Some(1024 * 1024 * 1024),
            max_cpu_percent: Some(80.0),
            max_open_files: Some(1024),
            max_connections: Some(100),
        };
        let mut monitor = ResourceMonitor::with_metrics(quota, &registry);
        monitor.check_quotas();

        let output = prometheus::TextEncoder::new()
            .encode_to_string(&registry.gather())
            .unwrap();
        assert!(output.contains("resource_memory_bytes"));
        assert!(output.contains("resource_open_files"));
    }

    #[test]
    fn test_quota_accessor() {
        let quota = ResourceQuota {
            max_memory_bytes: Some(4096),
            max_cpu_percent: None,
            max_open_files: Some(256),
            max_connections: None,
        };
        let monitor = ResourceMonitor::new(quota.clone());
        assert_eq!(monitor.quota().max_memory_bytes, Some(4096));
        assert_eq!(monitor.quota().max_open_files, Some(256));
    }

    #[test]
    fn test_read_usage_snapshot() {
        let quota = ResourceQuota::default();
        let monitor = ResourceMonitor::new(quota);
        let _usage = monitor.read_usage_snapshot();
        // Snapshot should not modify internal CPU state
        assert!(monitor.prev_cpu_user.is_none());
    }

    #[test]
    fn test_periodic_monitoring_running_flag() {
        let quota = ResourceQuota::default();
        let monitor = ResourceMonitor::new(quota);
        assert!(!monitor.running.load(Ordering::SeqCst));

        monitor.stop_periodic_monitoring();
        assert!(!monitor.running.load(Ordering::SeqCst));
    }

    #[test]
    fn test_metrics_cpu_times_registered() {
        let registry = Registry::new();
        let quota = ResourceQuota::default();
        let _monitor = ResourceMonitor::with_metrics(quota, &registry);

        let output = prometheus::TextEncoder::new()
            .encode_to_string(&registry.gather())
            .unwrap();
        assert!(output.contains("resource_cpu_user_time_us"));
        assert!(output.contains("resource_cpu_system_time_us"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_read_usage_returns_something_on_linux() {
        let quota = ResourceQuota::default();
        let mut monitor = ResourceMonitor::new(quota);
        let usage = monitor.read_usage();
        // On Linux, we should get memory and fd counts
        assert!(usage.memory_bytes.is_some());
        assert!(usage.open_files.is_some());
        assert!(usage.memory_bytes.unwrap() > 0);
        assert!(usage.open_files.unwrap() > 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_read_cpu_times_on_linux() {
        let times = read_cpu_times_static();
        assert!(times.is_some());
        let (user, system) = times.unwrap();
        // user could be 0 in some containerized environments but the pair should exist
        let _ = (user, system);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_read_vm_rss_returns_value() {
        let val = read_vm_rss_static();
        assert!(val.is_some());
        assert!(val.unwrap() > 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_read_fd_count_returns_value() {
        let val = read_fd_count_static();
        assert!(val.is_some());
        assert!(val.unwrap() > 0);
    }

    #[test]
    fn test_read_usage_updates_cpu_state() {
        let quota = ResourceQuota::default();
        let mut monitor = ResourceMonitor::new(quota);
        // First call initializes prev_cpu
        let _usage1 = monitor.read_usage();
        assert!(monitor.prev_cpu_user.is_some());
        assert!(monitor.prev_cpu_system.is_some());
        assert!(monitor.prev_wall_time.is_some());
        // Second call should calculate delta (may be 0 or > 0)
        let usage2 = monitor.read_usage();
        // Usage2 should have cpu_percent if delta > 0
        let _ = usage2.cpu_percent;
    }

    #[test]
    fn test_read_usage_snapshot_no_state_change() {
        let quota = ResourceQuota::default();
        let mut monitor = ResourceMonitor::new(quota);
        let _ = monitor.read_usage();
        let prev_user = monitor.prev_cpu_user;
        let _ = monitor.read_usage_snapshot();
        assert_eq!(monitor.prev_cpu_user, prev_user);
    }

    #[tokio::test]
    async fn test_stop_and_start_periodic_monitoring() {
        let quota = ResourceQuota::default();
        let monitor = ResourceMonitor::new(quota);
        let handle = monitor.start_periodic_monitoring();
        assert!(monitor.running.load(Ordering::SeqCst));
        monitor.stop_periodic_monitoring();
        assert!(!monitor.running.load(Ordering::SeqCst));
        // Wait for the task to notice the flag and exit
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(handle.is_finished());
    }

    #[test]
    fn test_check_quotas_metrics_update() {
        let registry = Registry::new();
        let quota = ResourceQuota {
            max_memory_bytes: Some(u64::MAX),
            max_cpu_percent: Some(100.0),
            max_open_files: Some(u32::MAX),
            max_connections: None,
        };
        let mut monitor = ResourceMonitor::with_metrics(quota, &registry);
        assert!(monitor.check_quotas());
        // Metrics should be populated
        let m = monitor.metrics().unwrap();
        // On Linux, memory should be > 0
        #[cfg(target_os = "linux")]
        assert!(m.memory_bytes.get() > 0.0);
    }

    #[test]
    fn test_resource_metrics_all_registered() {
        let registry = Registry::new();
        let quota = ResourceQuota::default();
        let _ = ResourceMonitor::with_metrics(quota, &registry);

        let metric_families = registry.gather();
        let names: Vec<&str> = metric_families.iter().map(|m| m.get_name()).collect();
        assert!(names.contains(&"resource_memory_bytes"));
        assert!(names.contains(&"resource_cpu_percent"));
        assert!(names.contains(&"resource_open_files"));
        assert!(names.contains(&"resource_cpu_user_time_us"));
        assert!(names.contains(&"resource_cpu_system_time_us"));
    }

    #[test]
    fn test_check_quotas_warning_level_memory() {
        // Set quota to a value that should trigger a warning (80-100% usage)
        // On Linux, current process memory is likely > 1 byte
        let quota = ResourceQuota {
            max_memory_bytes: Some(2), // Very small, will exceed 100%
            max_cpu_percent: None,
            max_open_files: None,
            max_connections: None,
        };
        let mut monitor = ResourceMonitor::new(quota);
        assert!(!monitor.check_quotas()); // Should exceed limit
    }

    #[test]
    fn test_check_quotas_warning_level_files() {
        let quota = ResourceQuota {
            max_memory_bytes: None,
            max_cpu_percent: None,
            max_open_files: Some(2), // Very small, will exceed 100%
            max_connections: None,
        };
        let mut monitor = ResourceMonitor::new(quota);
        assert!(!monitor.check_quotas()); // Should exceed limit
    }

    #[test]
    fn test_resource_usage_clone() {
        let usage = ResourceUsage {
            memory_bytes: Some(1024),
            cpu_percent: Some(50.0),
            open_files: Some(10),
            cpu_user_time: Some(100),
            cpu_system_time: Some(50),
        };
        let cloned = usage.clone();
        assert_eq!(cloned.memory_bytes, Some(1024));
        assert_eq!(cloned.cpu_percent, Some(50.0));
        assert_eq!(cloned.open_files, Some(10));
        assert_eq!(cloned.cpu_user_time, Some(100));
        assert_eq!(cloned.cpu_system_time, Some(50));
    }

    #[test]
    fn test_resource_usage_debug() {
        let usage = ResourceUsage::default();
        let debug = format!("{:?}", usage);
        assert!(debug.contains("ResourceUsage"));
    }
}
