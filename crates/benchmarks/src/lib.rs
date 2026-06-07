//! Benchmark regression detection for EvergreenShims.
//!
//! Parses Criterion output, compares against baselines, and reports
//! regressions that exceed configurable tolerance thresholds.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

/// Baseline file containing known-good benchmark results.
#[derive(Debug, Serialize, Deserialize)]
pub struct BaselineFile {
    /// Schema version of the baseline file.
    pub version: u32,
    /// Map of benchmark name to its baseline measurement.
    pub benchmarks: HashMap<String, BenchmarkBaseline>,
}

/// A single benchmark baseline measurement.
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkBaseline {
    /// Mean execution time in nanoseconds.
    pub mean_ns: f64,
    /// Allowed regression tolerance as a fraction (e.g., 0.1 = 10%).
    pub tolerance: f64,
}

/// A parsed benchmark result from Criterion output.
#[derive(Debug)]
pub struct BenchmarkResult {
    /// Benchmark name.
    pub name: String,
    /// Measured mean execution time in nanoseconds.
    pub mean_ns: f64,
}

/// Summary of regression check results.
#[derive(Debug)]
pub struct RegressionReport {
    /// Benchmarks that passed regression checks.
    pub passed: Vec<String>,
    /// Benchmarks that failed (regressed beyond tolerance).
    pub failed: Vec<RegressionFailure>,
}

/// A single benchmark regression failure.
#[derive(Debug)]
pub struct RegressionFailure {
    /// Benchmark name.
    pub name: String,
    /// Baseline mean time in nanoseconds.
    pub baseline_ns: f64,
    /// Measured mean time in nanoseconds.
    pub measured_ns: f64,
    /// Percentage regression (positive = slower).
    pub regression_pct: f64,
    /// Allowed tolerance as a fraction.
    pub tolerance: f64,
}

pub fn load_baseline(path: &str) -> Result<BaselineFile, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let baseline: BaselineFile = serde_json::from_str(&content)?;
    Ok(baseline)
}

pub fn parse_criterion_line(line: &str) -> Option<BenchmarkResult> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with("Benchmarking") {
        return None;
    }

    if let Some(pos) = line.find("time:") {
        let name_part = line[..pos].trim();
        let time_part = line[pos + 5..].trim();

        let name = name_part.to_string();

        let ns = parse_time_to_ns(time_part)?;
        Some(BenchmarkResult { name, mean_ns: ns })
    } else {
        None
    }
}

fn parse_time_to_ns(time_str: &str) -> Option<f64> {
    let time_str = time_str.trim();

    // Strip leading '[' if present (criterion format: [1.23 ms 1.24 ms 1.25 ms])
    let time_str = time_str.strip_prefix('[').unwrap_or(time_str);

    let parts: Vec<&str> = time_str.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let value_str = parts[0].replace(',', "");
    let value: f64 = value_str.parse().ok()?;

    let unit = if parts.len() > 1 { parts[1] } else { "ns" };

    let multiplier = match unit {
        "ns" => 1.0,
        "µs" | "us" => 1_000.0,
        "ms" => 1_000_000.0,
        "s" => 1_000_000_000.0,
        _ => return None,
    };

    Some(value * multiplier)
}

pub fn compare_results(baseline: &BaselineFile, results: &[BenchmarkResult]) -> RegressionReport {
    let mut passed = Vec::new();
    let mut failed = Vec::new();

    for result in results {
        if let Some(baseline_entry) = baseline.benchmarks.get(&result.name) {
            let tolerance = baseline_entry.tolerance;
            let threshold = baseline_entry.mean_ns * (1.0 + tolerance);

            if result.mean_ns > threshold {
                let regression_pct =
                    ((result.mean_ns - baseline_entry.mean_ns) / baseline_entry.mean_ns) * 100.0;
                failed.push(RegressionFailure {
                    name: result.name.clone(),
                    baseline_ns: baseline_entry.mean_ns,
                    measured_ns: result.mean_ns,
                    regression_pct,
                    tolerance,
                });
            } else {
                passed.push(result.name.clone());
            }
        } else {
            passed.push(result.name.clone());
        }
    }

    RegressionReport { passed, failed }
}

pub fn parse_criterion_output(output: &str) -> Vec<BenchmarkResult> {
    output.lines().filter_map(parse_criterion_line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_baseline(benchmarks: Vec<(&str, f64, f64)>) -> BaselineFile {
        let mut map = HashMap::new();
        for (name, mean_ns, tolerance) in benchmarks {
            map.insert(name.to_string(), BenchmarkBaseline { mean_ns, tolerance });
        }
        BaselineFile {
            version: 1,
            benchmarks: map,
        }
    }

    #[test]
    fn test_parse_criterion_line_standard() {
        let line = "backup_checksum_1mb      time:   [1.2345 ms 1.2567 ms 1.2789 ms]";
        let result = parse_criterion_line(line).unwrap();
        assert_eq!(result.name, "backup_checksum_1mb");
        assert!((result.mean_ns - 1_234_500.0).abs() < 1000.0);
    }

    #[test]
    fn test_parse_criterion_line_nanoseconds() {
        let line = "migration_checksum       time:   [450.00 ns 460.00 ns 470.00 ns]";
        let result = parse_criterion_line(line).unwrap();
        assert_eq!(result.name, "migration_checksum");
        assert!((result.mean_ns - 450.0).abs() < 1.0);
    }

    #[test]
    fn test_parse_criterion_line_microseconds() {
        let line = "aes_gcm_encrypt         time:   [120.50 µs 125.30 µs 130.10 µs]";
        let result = parse_criterion_line(line).unwrap();
        assert_eq!(result.name, "aes_gcm_encrypt");
        assert!((result.mean_ns - 120_500.0).abs() < 1000.0);
    }

    #[test]
    fn test_parse_criterion_line_grouped() {
        let line = "cache_operations/set_1000_keys  time:   [5.1234 ms 5.2345 ms 5.3456 ms]";
        let result = parse_criterion_line(line).unwrap();
        assert_eq!(result.name, "cache_operations/set_1000_keys");
        assert!((result.mean_ns - 5_123_400.0).abs() < 10_000.0);
    }

    #[test]
    fn test_parse_criterion_line_empty() {
        assert!(parse_criterion_line("").is_none());
    }

    #[test]
    fn test_parse_criterion_line_comment() {
        assert!(parse_criterion_line("# some comment").is_none());
    }

    #[test]
    fn test_parse_criterion_line_benchmarking() {
        assert!(parse_criterion_line("Benchmarking backup_checksum_1mb").is_none());
    }

    #[test]
    fn test_parse_criterion_line_no_time() {
        assert!(parse_criterion_line("some random output").is_none());
    }

    #[test]
    fn test_compare_results_within_tolerance() {
        let baseline = make_baseline(vec![("bench_a", 1_000_000.0, 0.20)]);
        let results = vec![BenchmarkResult {
            name: "bench_a".to_string(),
            mean_ns: 1_150_000.0, // 15% regression, within 20% tolerance
        }];

        let report = compare_results(&baseline, &results);
        assert_eq!(report.failed.len(), 0);
        assert_eq!(report.passed.len(), 1);
    }

    #[test]
    fn test_compare_results_exceeds_tolerance() {
        let baseline = make_baseline(vec![("bench_a", 1_000_000.0, 0.20)]);
        let results = vec![BenchmarkResult {
            name: "bench_a".to_string(),
            mean_ns: 1_250_000.0, // 25% regression, exceeds 20% tolerance
        }];

        let report = compare_results(&baseline, &results);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.passed.len(), 0);
        assert!((report.failed[0].regression_pct - 25.0).abs() < 0.1);
    }

    #[test]
    fn test_compare_results_exactly_at_boundary() {
        let baseline = make_baseline(vec![("bench_a", 1_000_000.0, 0.20)]);
        let results = vec![BenchmarkResult {
            name: "bench_a".to_string(),
            mean_ns: 1_200_000.0, // Exactly 20% regression
        }];

        let report = compare_results(&baseline, &results);
        assert_eq!(report.failed.len(), 0, "Should pass at exactly 20%");
    }

    #[test]
    fn test_compare_results_improvement() {
        let baseline = make_baseline(vec![("bench_a", 1_000_000.0, 0.20)]);
        let results = vec![BenchmarkResult {
            name: "bench_a".to_string(),
            mean_ns: 800_000.0, // 20% improvement
        }];

        let report = compare_results(&baseline, &results);
        assert_eq!(report.failed.len(), 0);
        assert_eq!(report.passed.len(), 1);
    }

    #[test]
    fn test_compare_results_unknown_benchmark() {
        let baseline = make_baseline(vec![("bench_a", 1_000_000.0, 0.20)]);
        let results = vec![BenchmarkResult {
            name: "bench_unknown".to_string(),
            mean_ns: 5_000_000.0,
        }];

        let report = compare_results(&baseline, &results);
        assert_eq!(report.failed.len(), 0, "Unknown benchmarks should pass");
        assert_eq!(report.passed.len(), 1);
    }

    #[test]
    fn test_compare_results_multiple_benchmarks() {
        let baseline = make_baseline(vec![
            ("bench_fast", 100_000.0, 0.20),
            ("bench_slow", 1_000_000.0, 0.20),
        ]);
        let results = vec![
            BenchmarkResult {
                name: "bench_fast".to_string(),
                mean_ns: 110_000.0, // 10% regression
            },
            BenchmarkResult {
                name: "bench_slow".to_string(),
                mean_ns: 1_300_000.0, // 30% regression
            },
        ];

        let report = compare_results(&baseline, &results);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.passed.len(), 1);
        assert_eq!(report.failed[0].name, "bench_slow");
    }

    #[test]
    fn test_parse_criterion_output_full() {
        let output = r#"backup_checksum_1mb      time:   [1.2345 ms 1.2567 ms 1.2789 ms]
cache_operations/set_1000_keys  time:   [5.1234 ms 5.2345 ms 5.3456 ms]
cache_operations/get_1000_keys  time:   [3.4567 ms 3.5678 ms 3.6789 ms]
encryption_throughput/aes_gcm_encrypt_4kb  time:   [800.00 ns 820.00 ns 840.00 ns]
encryption_throughput/aes_gcm_decrypt_4kb  time:   [700.00 ns 710.00 ns 720.00 ns]
migration_checksum       time:   [450.00 ns 460.00 ns 470.00 ns]"#;

        let results = parse_criterion_output(output);
        assert_eq!(results.len(), 6);
        assert_eq!(results[0].name, "backup_checksum_1mb");
        assert_eq!(results[5].name, "migration_checksum");
    }

    #[test]
    fn test_regression_failure_message() {
        let baseline = make_baseline(vec![("bench_a", 1_000_000.0, 0.20)]);
        let results = vec![BenchmarkResult {
            name: "bench_a".to_string(),
            mean_ns: 1_500_000.0,
        }];

        let report = compare_results(&baseline, &results);
        assert_eq!(report.failed.len(), 1);
        assert!((report.failed[0].regression_pct - 50.0).abs() < 0.1);
        assert!((report.failed[0].baseline_ns - 1_000_000.0).abs() < 1.0);
        assert!((report.failed[0].measured_ns - 1_500_000.0).abs() < 1.0);
    }
}
