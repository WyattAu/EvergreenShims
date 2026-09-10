use crate::error::TelemetryError;
use crate::exporter::Exporter;

/// Output format for structured logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable text output.
    Text,
    /// Machine-readable JSON output.
    Json,
}

impl LogFormat {
    /// Parse a format string, defaulting to [`LogFormat::Json`].
    pub fn from_str_opt(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "text" | "plain" => LogFormat::Text,
            _ => LogFormat::Json,
        }
    }
}

/// Configuration for telemetry initialization.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// The logical name of the service.
    pub service_name: String,
    /// The version of the service.
    pub service_version: String,
    /// Log level filter string (e.g. `"info"`, `"debug,hyper=warn"`).
    pub log_level: String,
    /// Log output format.
    pub log_format: LogFormat,
    /// OTLP exporter endpoint, if any.
    pub otlp_endpoint: Option<String>,
    /// Sentry DSN, if any.
    pub sentry_dsn: Option<String>,
    /// Trace sample rate (0.0 – 1.0).
    pub sample_rate: f32,
    /// Exporter backend selection.
    ///
    /// Defaults to [`Exporter::Otlp`], which preserves the historical
    /// behavior. Added in 2.0.0.
    pub exporter: Exporter,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            service_name: "unknown".into(),
            service_version: "0.0.0".into(),
            log_level: "info".into(),
            log_format: LogFormat::Json,
            otlp_endpoint: None,
            sentry_dsn: None,
            sample_rate: 1.0,
            exporter: Exporter::default(),
        }
    }
}

impl TelemetryConfig {
    /// Create a config populated from environment variables.
    pub fn from_env() -> Result<Self, TelemetryError> {
        Ok(Self {
            service_name: std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "unknown".into()),
            service_version: std::env::var("OTEL_SERVICE_VERSION")
                .unwrap_or_else(|_| "0.0.0".into()),
            log_level: std::env::var("OTEL_LOG_LEVEL").unwrap_or_else(|_| "info".into()),
            log_format: LogFormat::from_str_opt(
                &std::env::var("OTEL_LOG_FORMAT").unwrap_or_else(|_| "json".into()),
            ),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            sentry_dsn: std::env::var("SENTRY_DSN").ok(),
            sample_rate: std::env::var("OTEL_SAMPLE_RATE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            exporter: Exporter::from_str_opt(
                &std::env::var("OTEL_EXPORTER").unwrap_or_else(|_| "otlp".into()),
            ),
        })
    }

    /// Create a config with the given service name and defaults.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self::default().service_name(service_name)
    }

    /// Set the service name.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Set the service version.
    pub fn service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = version.into();
        self
    }

    /// Set the log level filter.
    pub fn log_level(mut self, level: impl Into<String>) -> Self {
        self.log_level = level.into();
        self
    }

    /// Set the log format.
    pub fn log_format(mut self, format: LogFormat) -> Self {
        self.log_format = format;
        self
    }

    /// Set the OTLP endpoint.
    pub fn otlp_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(endpoint.into());
        self
    }

    /// Set the Sentry DSN.
    pub fn sentry_dsn(mut self, dsn: impl Into<String>) -> Self {
        self.sentry_dsn = Some(dsn.into());
        self
    }

    /// Set the trace sample rate.
    pub fn sample_rate(mut self, rate: f32) -> Self {
        self.sample_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Set the telemetry exporter backend.
    pub fn exporter(mut self, exporter: Exporter) -> Self {
        self.exporter = exporter;
        self
    }
}
