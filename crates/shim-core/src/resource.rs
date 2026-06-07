//! Resource quota monitoring for shim processes.
//!
//! `ResourceMonitor` tracks current resource usage by reading `/proc/self/status`
//! on Linux and checks against configured `ResourceQuota` limits. It logs warnings
//! when usage exceeds 80% of any configured quota.

use prometheus::{Gauge, Registry};
use tracing::{error, warn};

use crate::config::ResourceQuota;

/// Current resource usage snapshot.
#[derive(Debug, Clone, Default)]
pub struct ResourceUsage {
    /// Memory usage in bytes (from VmRSS).
    pub memory_bytes: Option<u64>,
    /// CPU usage percentage (estimated, not directly available from /proc).
    pub cpu_percent: Option<f64>,
    /// Number of open file descriptors (from FDSize or /proc/self/fd count).
    pub open_files: Option<u32>,
}

/// Metrics for resource monitoring.
pub struct ResourceMetrics {
    /// Current memory usage in bytes.
    pub memory_bytes: Gauge,
    /// Current CPU usage percent.
    pub cpu_percent: Gauge,
    /// Current open file descriptor count.
    pub open_files: Gauge,
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

        registry
            .register(Box::new(memory_bytes.clone()))
            .expect("register memory_bytes must not conflict");
        registry
            .register(Box::new(cpu_percent.clone()))
            .expect("register cpu_percent must not conflict");
        registry
            .register(Box::new(open_files.clone()))
            .expect("register open_files must not conflict");

        Self {
            memory_bytes,
            cpu_percent,
            open_files,
        }
    }
}

/// Monitors resource usage and checks against configured quotas.
pub struct ResourceMonitor {
    /// Configured resource quotas.
    quota: ResourceQuota,
    /// Optional Prometheus metrics.
    metrics: Option<ResourceMetrics>,
}

impl ResourceMonitor {
    /// Create a new resource monitor with the given quota.
    pub fn new(quota: ResourceQuota) -> Self {
        Self {
            quota,
            metrics: None,
        }
    }

    /// Create a new resource monitor with metrics collection.
    pub fn with_metrics(quota: ResourceQuota, registry: &Registry) -> Self {
        Self {
            quota,
            metrics: Some(ResourceMetrics::new(registry)),
        }
    }

    /// Read current resource usage from the OS.
    ///
    /// On Linux, parses `/proc/self/status` for VmRSS and FDSize.
    /// Falls back to empty values on unsupported platforms.
    pub fn read_usage(&self) -> ResourceUsage {
        let mut usage = ResourceUsage::default();

        #[cfg(target_os = "linux")]
        {
            usage.memory_bytes = self.read_vm_rss();
            usage.open_files = self.read_fd_count();
        }

        // CPU percent is not directly available from /proc; callers should
        // compute it by sampling over time. We expose the gauge for external
        // population.
        usage.cpu_percent = None;

        usage
    }

    /// Check current usage against quotas and log warnings if thresholds exceeded.
    ///
    /// Returns `true` if all quotas are within bounds, `false` if any quota is exceeded.
    pub fn check_quotas(&self) -> bool {
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

    #[cfg(target_os = "linux")]
    fn read_fd_count(&self) -> Option<u32> {
        std::fs::read_dir("/proc/self/fd")
            .ok()
            .map(|entries| entries.count() as u32)
    }
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
        let monitor = ResourceMonitor::new(quota);
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
        let monitor = ResourceMonitor::new(quota);
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
        let monitor = ResourceMonitor::new(quota);
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
        let monitor = ResourceMonitor::new(quota);
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
        let monitor = ResourceMonitor::with_metrics(quota, &registry);
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

    #[cfg(target_os = "linux")]
    #[test]
    fn test_read_usage_returns_something_on_linux() {
        let quota = ResourceQuota::default();
        let monitor = ResourceMonitor::new(quota);
        let usage = monitor.read_usage();
        // On Linux, we should get memory and fd counts
        assert!(usage.memory_bytes.is_some());
        assert!(usage.open_files.is_some());
        assert!(usage.memory_bytes.unwrap() > 0);
        assert!(usage.open_files.unwrap() > 0);
    }
}
