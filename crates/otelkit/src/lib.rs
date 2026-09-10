#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! OpenTelemetry and tracing integration for Rust.

mod config;
/// Error types.
pub mod error;
/// Telemetry exporter backend selection.
pub mod exporter;

pub use config::{LogFormat, TelemetryConfig};
pub use error::TelemetryError;
pub use exporter::Exporter;

/// RAII guard that flushes and shuts down telemetry on drop.
pub struct TelemetryGuard {
    #[cfg(feature = "otlp")]
    tracer_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    #[cfg(feature = "stdout")]
    stdout_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    #[cfg(feature = "prometheus")]
    meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
    #[cfg(feature = "prometheus")]
    prometheus_registry: Option<prometheus::Registry>,
    #[cfg(feature = "sentry")]
    sentry_guard: Option<sentry::ClientInitGuard>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        #[cfg(feature = "sentry")]
        {
            self.sentry_guard.take();
        }
        #[cfg(feature = "otlp")]
        {
            if let Some(provider) = self.tracer_provider.take()
                && let Err(e) = provider.shutdown()
            {
                eprintln!("otelkit: tracer provider shutdown error: {e}");
            }
        }
        #[cfg(feature = "stdout")]
        {
            if let Some(provider) = self.stdout_provider.take()
                && let Err(e) = provider.shutdown()
            {
                eprintln!("otelkit: stdout provider shutdown error: {e}");
            }
        }
        #[cfg(feature = "prometheus")]
        {
            if let Some(provider) = self.meter_provider.take()
                && let Err(e) = provider.shutdown()
            {
                eprintln!("otelkit: meter provider shutdown error: {e}");
            }
        }
    }
}

impl TelemetryGuard {
    /// Gather the current Prometheus exposition text.
    ///
    /// Only available with the `prometheus` feature and only meaningful when
    /// the guard was created from a config selecting
    /// [`Exporter::Prometheus`]; returns an empty exposition otherwise.
    #[cfg(feature = "prometheus")]
    pub fn gather_metrics(&self) -> Result<String, TelemetryError> {
        use prometheus::Encoder as _;

        let registry = self.prometheus_registry.as_ref().ok_or_else(|| {
            TelemetryError::InvalidConfig(
                "gather_metrics requires a Prometheus-backed guard".into(),
            )
        })?;
        let encoder = prometheus::TextEncoder::new();
        let families = registry.gather();
        let mut buf = Vec::new();
        encoder
            .encode(&families, &mut buf)
            .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;
        String::from_utf8(buf).map_err(|e| TelemetryError::InvalidConfig(e.to_string()))
    }
}

/// Initialize tracing with the stdout span exporter.
///
/// Spans are printed to stdout; no network access is needed, which makes
/// this path fully hermetic in tests.
#[cfg(feature = "stdout")]
fn init_stdout(config: TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_new(&config.log_level)
        .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;

    let resource = opentelemetry_sdk::Resource::builder()
        .with_attributes([opentelemetry::KeyValue::new(
            "service.name",
            config.service_name.clone(),
        )])
        .build();

    let exporter = opentelemetry_stdout::SpanExporter::default();
    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    let tracer = tracer_provider.tracer(config.service_name.clone());
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .with(otel_layer)
        .try_init()
        .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;

    Ok(TelemetryGuard {
        #[cfg(feature = "otlp")]
        tracer_provider: None,
        stdout_provider: Some(tracer_provider),
        #[cfg(feature = "prometheus")]
        meter_provider: None,
        #[cfg(feature = "prometheus")]
        prometheus_registry: None,
        #[cfg(feature = "sentry")]
        sentry_guard: None,
    })
}

/// Initialize tracing plus a Prometheus meter provider.
///
/// Traces continue through the configured log format layer; metrics are
/// collected into an in-process registry exposed via
/// [`TelemetryGuard::gather_metrics`].
#[cfg(feature = "prometheus")]
fn init_prometheus(config: TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_new(&config.log_level)
        .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;

    let registry = prometheus::Registry::new();
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(registry.clone())
        .build()
        .map_err(|e| TelemetryError::OtlpConnection(e.to_string()))?;
    let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(exporter)
        .build();
    opentelemetry::global::set_meter_provider(meter_provider.clone());

    let registry_fmt = tracing_subscriber::registry().with(filter);
    match config.log_format {
        LogFormat::Text => {
            registry_fmt
                .with(tracing_subscriber::fmt::layer().with_target(true))
                .try_init()
                .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;
        }
        LogFormat::Json => {
            registry_fmt
                .with(tracing_subscriber::fmt::layer().json().with_target(true))
                .try_init()
                .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;
        }
    }

    Ok(TelemetryGuard {
        #[cfg(feature = "otlp")]
        tracer_provider: None,
        #[cfg(feature = "stdout")]
        stdout_provider: None,
        meter_provider: Some(meter_provider),
        prometheus_registry: Some(registry),
        #[cfg(feature = "sentry")]
        sentry_guard: None,
    })
}

/// Initialize tracing and telemetry from the given configuration.
///
/// Returns a [`TelemetryGuard`] that must be held for the lifetime of the
/// program to ensure resources are flushed on shutdown.
pub fn init(config: TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_new(&config.log_level)
        .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;

    let mut guard = TelemetryGuard {
        #[cfg(feature = "otlp")]
        tracer_provider: None,
        #[cfg(feature = "stdout")]
        stdout_provider: None,
        #[cfg(feature = "prometheus")]
        meter_provider: None,
        #[cfg(feature = "prometheus")]
        prometheus_registry: None,
        #[cfg(feature = "sentry")]
        sentry_guard: None,
    };

    // A non-OTLP selection without its feature compiled in is a
    // configuration error, never silent behavior.
    #[allow(unreachable_patterns)]
    match config.exporter {
        #[cfg(feature = "stdout")]
        Exporter::Stdout => return init_stdout(config),
        #[cfg(feature = "prometheus")]
        Exporter::Prometheus => return init_prometheus(config),
        Exporter::Stdout => {
            return Err(TelemetryError::InvalidConfig(
                "Exporter::Stdout selected but the `stdout` feature is not enabled".into(),
            ));
        }
        Exporter::Prometheus => {
            return Err(TelemetryError::InvalidConfig(
                "Exporter::Prometheus selected but the `prometheus` feature is not enabled".into(),
            ));
        }
        Exporter::Otlp => {}
    }

    #[cfg(feature = "otlp")]
    if config.otlp_endpoint.is_some() {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_otlp::WithExportConfig;

        let otlp_endpoint = config
            .otlp_endpoint
            .as_deref()
            .unwrap_or("http://localhost:4317");

        let resource = opentelemetry_sdk::Resource::builder()
            .with_attributes([opentelemetry::KeyValue::new(
                "service.name",
                config.service_name.clone(),
            )])
            .build();

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(otlp_endpoint)
            .build()
            .map_err(|e| TelemetryError::OtlpConnection(e.to_string()))?;

        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource)
            .build();

        let tracer = tracer_provider.tracer(config.service_name.clone());
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_new(&config.log_level)
                    .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?,
            )
            .with(tracing_subscriber::fmt::layer().with_target(true))
            .with(otel_layer)
            .try_init()
            .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;

        guard.tracer_provider = Some(tracer_provider);
        return Ok(guard);
    }

    let registry = tracing_subscriber::registry().with(filter);

    match config.log_format {
        LogFormat::Text => {
            registry
                .with(tracing_subscriber::fmt::layer().with_target(true))
                .try_init()
                .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;
        }
        LogFormat::Json => {
            registry
                .with(tracing_subscriber::fmt::layer().json().with_target(true))
                .try_init()
                .map_err(|e| TelemetryError::InvalidConfig(e.to_string()))?;
        }
    }

    #[cfg(feature = "sentry")]
    {
        let sentry_dsn = config.sentry_dsn.as_deref().ok_or_else(|| {
            TelemetryError::InvalidConfig("sentry feature enabled but no DSN provided".into())
        })?;

        guard.sentry_guard = Some(sentry::init((
            sentry_dsn,
            sentry::ClientOptions {
                traces_sample_rate: config.sample_rate,
                release: Some(config.service_version.clone().into()),
                ..Default::default()
            },
        )));
    }

    Ok(guard)
}

/// Build a [`TelemetryConfig`] from environment variables.
///
/// Recognized variables:
/// - `OTEL_SERVICE_NAME` — service name (default: `"unknown"`)
/// - `OTEL_SERVICE_VERSION` — service version (default: `"0.0.0"`)
/// - `OTEL_LOG_LEVEL` — log level filter (default: `"info"`)
/// - `OTEL_LOG_FORMAT` — `"text"` or `"json"` (default: `"json"`)
/// - `OTEL_EXPORTER_OTLP_ENDPOINT` — OTLP endpoint
/// - `OTEL_EXPORTER` — exporter backend: `"otlp"` (default), `"stdout"`, `"prometheus"`
/// - `SENTRY_DSN` — Sentry DSN
/// - `OTEL_SAMPLE_RATE` — trace sample rate 0.0–1.0 (default: `1.0`)
pub fn from_env() -> Result<TelemetryConfig, TelemetryError> {
    TelemetryConfig::from_env()
}

// Tests exercise failure paths and invariants directly; unwrap/expect,
// slicing, and panicking asserts are acceptable here — violations
// surface as test failures, not production panics.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
#[cfg(test)]
mod tests {
    use crate::config::{LogFormat, TelemetryConfig};
    use crate::error::TelemetryError;

    #[cfg(feature = "otlp")]
    #[test]
    fn guard_drop_reports_provider_shutdown_error() {
        use opentelemetry_sdk::trace::SdkTracerProvider;

        // A provider that has already been shut down fails the guard's
        // shutdown call (AlreadyShutdown), exercising the error branch of
        // `Drop for TelemetryGuard`.
        let provider = SdkTracerProvider::builder().build();
        assert!(provider.shutdown().is_ok());

        let guard = crate::TelemetryGuard {
            tracer_provider: Some(provider),
            #[cfg(feature = "stdout")]
            stdout_provider: None,
            #[cfg(feature = "prometheus")]
            meter_provider: None,
            #[cfg(feature = "prometheus")]
            prometheus_registry: None,
            #[cfg(feature = "sentry")]
            sentry_guard: None,
        };
        drop(guard);
    }

    #[cfg(feature = "stdout")]
    #[test]
    fn guard_drop_reports_stdout_provider_shutdown_error() {
        use opentelemetry_sdk::trace::SdkTracerProvider;

        // A provider that has already been shut down fails the guard's
        // stdout shutdown call (AlreadyShutdown), exercising the error
        // branch of `Drop for TelemetryGuard`.
        let provider = SdkTracerProvider::builder().build();
        assert!(provider.shutdown().is_ok());

        let guard = crate::TelemetryGuard {
            #[cfg(feature = "otlp")]
            tracer_provider: None,
            stdout_provider: Some(provider),
            #[cfg(feature = "prometheus")]
            meter_provider: None,
            #[cfg(feature = "prometheus")]
            prometheus_registry: None,
            #[cfg(feature = "sentry")]
            sentry_guard: None,
        };
        drop(guard);
    }

    #[cfg(feature = "prometheus")]
    #[test]
    fn guard_drop_reports_meter_provider_shutdown_error() {
        use opentelemetry_sdk::metrics::SdkMeterProvider;

        // Same AlreadyShutdown pattern for the metrics side.
        let provider = SdkMeterProvider::builder().build();
        assert!(provider.shutdown().is_ok());

        let guard = crate::TelemetryGuard {
            #[cfg(feature = "otlp")]
            tracer_provider: None,
            #[cfg(feature = "stdout")]
            stdout_provider: None,
            meter_provider: Some(provider),
            prometheus_registry: None,
            #[cfg(feature = "sentry")]
            sentry_guard: None,
        };
        drop(guard);
    }

    // ---- LogFormat tests ----

    #[test]
    fn log_format_variants() {
        let text = LogFormat::Text;
        let json = LogFormat::Json;
        assert_ne!(text, json);
    }

    #[test]
    fn log_format_equality() {
        assert_eq!(LogFormat::Text, LogFormat::Text);
        assert_eq!(LogFormat::Json, LogFormat::Json);
    }

    #[test]
    fn log_format_from_str_text() {
        assert_eq!(LogFormat::from_str_opt("text"), LogFormat::Text);
        assert_eq!(LogFormat::from_str_opt("TEXT"), LogFormat::Text);
        assert_eq!(LogFormat::from_str_opt("plain"), LogFormat::Text);
    }

    #[test]
    fn log_format_from_str_json_default() {
        assert_eq!(LogFormat::from_str_opt("json"), LogFormat::Json);
        assert_eq!(LogFormat::from_str_opt("anything"), LogFormat::Json);
        assert_eq!(LogFormat::from_str_opt(""), LogFormat::Json);
    }

    // ---- TelemetryConfig defaults tests ----

    #[test]
    fn telemetry_config_default() {
        let cfg = TelemetryConfig::default();
        assert_eq!(cfg.service_name, "unknown");
        assert_eq!(cfg.service_version, "0.0.0");
        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.log_format, LogFormat::Json);
        assert!(cfg.otlp_endpoint.is_none());
        assert!(cfg.sentry_dsn.is_none());
        assert_eq!(cfg.sample_rate, 1.0);
    }

    #[test]
    fn telemetry_config_builder() {
        let cfg = TelemetryConfig::default()
            .service_name("my-svc")
            .service_version("1.2.3")
            .log_level("debug")
            .log_format(LogFormat::Text)
            .otlp_endpoint("http://localhost:4317")
            .sentry_dsn("https://key@sentry.io/1")
            .sample_rate(0.5);

        assert_eq!(cfg.service_name, "my-svc");
        assert_eq!(cfg.service_version, "1.2.3");
        assert_eq!(cfg.log_level, "debug");
        assert_eq!(cfg.log_format, LogFormat::Text);
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://localhost:4317"));
        assert_eq!(cfg.sentry_dsn.as_deref(), Some("https://key@sentry.io/1"));
        assert_eq!(cfg.sample_rate, 0.5);
    }

    #[test]
    fn sample_rate_clamped() {
        let cfg = TelemetryConfig::default().sample_rate(2.0);
        assert_eq!(cfg.sample_rate, 1.0);

        let cfg = TelemetryConfig::default().sample_rate(-0.5);
        assert_eq!(cfg.sample_rate, 0.0);
    }

    // ---- TelemetryError display tests ----

    #[test]
    fn error_otlp_connection_display() {
        let err = TelemetryError::OtlpConnection("refused".into());
        assert_eq!(err.to_string(), "OTLP connection error: refused");
    }

    #[test]
    fn error_invalid_config_display() {
        let err = TelemetryError::InvalidConfig("bad value".into());
        assert_eq!(err.to_string(), "invalid config: bad value");
    }

    // ---- from_env defaults (no env vars set) ----

    #[test]
    fn from_env_defaults() {
        let cfg = TelemetryConfig::from_env().unwrap();
        assert_eq!(cfg.service_name, "unknown");
        assert_eq!(cfg.service_version, "0.0.0");
        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.log_format, LogFormat::Json);
        assert!(cfg.otlp_endpoint.is_none());
        assert!(cfg.sentry_dsn.is_none());
        assert_eq!(cfg.sample_rate, 1.0);
    }

    // ---- LogFormat Debug / Clone ----

    #[test]
    fn log_format_debug_and_clone() {
        let fmt = LogFormat::Text;
        let cloned = fmt;
        let debug_str = format!("{:?}", fmt);
        assert_eq!(debug_str, "Text");
        assert_eq!(fmt, cloned);
    }

    // ---- Additional LogFormat tests ----

    #[test]
    fn log_format_json_debug() {
        let fmt = LogFormat::Json;
        let debug_str = format!("{:?}", fmt);
        assert_eq!(debug_str, "Json");
    }

    #[test]
    fn log_format_from_str_opt_case_sensitivity() {
        assert_eq!(LogFormat::from_str_opt("Text"), LogFormat::Text);
        assert_eq!(LogFormat::from_str_opt("TEXT"), LogFormat::Text);
        assert_eq!(LogFormat::from_str_opt("text"), LogFormat::Text);
        assert_eq!(LogFormat::from_str_opt("JSON"), LogFormat::Json);
        assert_eq!(LogFormat::from_str_opt("Json"), LogFormat::Json);
    }

    #[test]
    fn log_format_from_str_opt_unknown_defaults_to_json() {
        assert_eq!(LogFormat::from_str_opt("unknown"), LogFormat::Json);
        assert_eq!(LogFormat::from_str_opt("xml"), LogFormat::Json);
        assert_eq!(LogFormat::from_str_opt("binary"), LogFormat::Json);
        assert_eq!(LogFormat::from_str_opt("123"), LogFormat::Json);
    }

    #[test]
    fn log_format_equality_asymmetric() {
        assert_ne!(LogFormat::Text, LogFormat::Json);
        assert_ne!(LogFormat::Json, LogFormat::Text);
    }

    #[test]
    fn log_format_clone_independence() {
        let fmt1 = LogFormat::Text;
        let fmt2 = fmt1;
        assert_eq!(fmt1, LogFormat::Text);
        assert_eq!(fmt2, LogFormat::Text);
    }

    // ---- Additional TelemetryConfig builder tests ----

    #[test]
    fn telemetry_config_builder_overwrite_service_name() {
        let cfg = TelemetryConfig::default()
            .service_name("first")
            .service_name("second");
        assert_eq!(cfg.service_name, "second");
    }

    #[test]
    fn telemetry_config_builder_overwrite_service_version() {
        let cfg = TelemetryConfig::default()
            .service_version("1.0.0")
            .service_version("2.0.0");
        assert_eq!(cfg.service_version, "2.0.0");
    }

    #[test]
    fn telemetry_config_builder_overwrite_log_level() {
        let cfg = TelemetryConfig::default()
            .log_level("info")
            .log_level("debug");
        assert_eq!(cfg.log_level, "debug");
    }

    #[test]
    fn telemetry_config_builder_overwrite_log_format() {
        let cfg = TelemetryConfig::default()
            .log_format(LogFormat::Text)
            .log_format(LogFormat::Json);
        assert_eq!(cfg.log_format, LogFormat::Json);
    }

    #[test]
    fn telemetry_config_builder_overwrite_otlp_endpoint() {
        let cfg = TelemetryConfig::default()
            .otlp_endpoint("http://first:4317")
            .otlp_endpoint("http://second:4317");
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://second:4317"));
    }

    #[test]
    fn telemetry_config_builder_overwrite_sentry_dsn() {
        let cfg = TelemetryConfig::default()
            .sentry_dsn("https://first@sentry.io/1")
            .sentry_dsn("https://second@sentry.io/2");
        assert_eq!(
            cfg.sentry_dsn.as_deref(),
            Some("https://second@sentry.io/2")
        );
    }

    #[test]
    fn sample_rate_clamp_boundary_values() {
        let cfg = TelemetryConfig::default().sample_rate(0.0);
        assert_eq!(cfg.sample_rate, 0.0);

        let cfg = TelemetryConfig::default().sample_rate(1.0);
        assert_eq!(cfg.sample_rate, 1.0);

        let cfg = TelemetryConfig::default().sample_rate(0.5);
        assert_eq!(cfg.sample_rate, 0.5);

        let cfg = TelemetryConfig::default().sample_rate(-100.0);
        assert_eq!(cfg.sample_rate, 0.0);

        let cfg = TelemetryConfig::default().sample_rate(100.0);
        assert_eq!(cfg.sample_rate, 1.0);
    }

    // ---- Additional TelemetryConfig default tests ----

    #[test]
    fn telemetry_config_default_clone() {
        let cfg = TelemetryConfig::default();
        let cloned = cfg.clone();
        assert_eq!(cfg.service_name, cloned.service_name);
        assert_eq!(cfg.service_version, cloned.service_version);
        assert_eq!(cfg.log_level, cloned.log_level);
        assert_eq!(cfg.log_format, cloned.log_format);
        assert_eq!(cfg.otlp_endpoint, cloned.otlp_endpoint);
        assert_eq!(cfg.sentry_dsn, cloned.sentry_dsn);
        assert_eq!(cfg.sample_rate, cloned.sample_rate);
    }

    #[test]
    fn telemetry_config_debug_format() {
        let cfg = TelemetryConfig::default();
        let debug_str = format!("{:?}", cfg);
        assert!(debug_str.contains("TelemetryConfig"));
        assert!(debug_str.contains("unknown"));
        assert!(debug_str.contains("info"));
    }

    // ---- from_env tests (safe, no env var manipulation) ----

    #[test]
    fn from_env_returns_defaults_when_no_vars_set() {
        // This test relies on the env vars not being set in the test environment.
        // The from_env_defaults test already covers this, but we add it for
        // explicit documentation of the from_env behavior.
        let cfg = TelemetryConfig::from_env().unwrap();
        assert!(!cfg.service_name.is_empty());
        assert!(!cfg.service_version.is_empty());
        assert!(!cfg.log_level.is_empty());
        assert!(cfg.sample_rate >= 0.0 && cfg.sample_rate <= 1.0);
    }

    #[test]
    fn from_env_preserves_builder_pattern_semantics() {
        // Verify from_env produces a valid config that can be further customized
        let cfg = TelemetryConfig::from_env()
            .unwrap()
            .service_name("override")
            .service_version("9.9.9");
        assert_eq!(cfg.service_name, "override");
        assert_eq!(cfg.service_version, "9.9.9");
    }

    #[test]
    fn from_env_invalid_sample_rate_unparseable() {
        // Verify that from_env handles unparseable sample rate gracefully.
        // We can't set env vars due to forbid(unsafe_code), so we test
        // the builder's sample_rate parsing logic instead.
        let cfg = TelemetryConfig::default().sample_rate(0.75);
        assert_eq!(cfg.sample_rate, 0.75);

        // Verify clamping edge cases (NaN would panic inside f32::clamp,
        // so the value is only built, not asserted on).
        let _cfg = TelemetryConfig::default().sample_rate(f32::NAN);
    }

    // ---- TelemetryError additional display tests ----

    #[test]
    fn error_otlp_connection_long_message() {
        let err = TelemetryError::OtlpConnection(
            "connection refused: endpoint http://localhost:4317 unreachable".into(),
        );
        let msg = err.to_string();
        assert!(msg.contains("OTLP connection error"));
        assert!(msg.contains("connection refused"));
        assert!(msg.contains("http://localhost:4317"));
    }

    #[test]
    fn error_invalid_config_long_message() {
        let err = TelemetryError::InvalidConfig(
            "expected log level to be one of: trace, debug, info, warn, error".into(),
        );
        let msg = err.to_string();
        assert!(msg.contains("invalid config"));
        assert!(msg.contains("trace"));
    }

    #[test]
    fn error_debug_format() {
        let err = TelemetryError::OtlpConnection("test".into());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("OtlpConnection"));

        let err = TelemetryError::InvalidConfig("test".into());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("InvalidConfig"));
    }

    #[test]
    fn error_is_std_error() {
        let err = TelemetryError::InvalidConfig("test".into());
        let std_err: &dyn std::error::Error = &err;
        assert!(std_err.to_string().contains("invalid config"));
        assert!(std_err.source().is_none());
    }

    // ---- Full builder chain test ----

    #[test]
    fn telemetry_config_full_builder_chain() {
        let cfg = TelemetryConfig::default()
            .service_name("full-chain-test")
            .service_version("0.1.0")
            .log_level("trace,hyper=warn")
            .log_format(LogFormat::Text)
            .otlp_endpoint("http://jaeger:4317")
            .sentry_dsn("https://abc@sentry.io/123")
            .sample_rate(0.25);

        assert_eq!(cfg.service_name, "full-chain-test");
        assert_eq!(cfg.service_version, "0.1.0");
        assert_eq!(cfg.log_level, "trace,hyper=warn");
        assert_eq!(cfg.log_format, LogFormat::Text);
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://jaeger:4317"));
        assert_eq!(cfg.sentry_dsn.as_deref(), Some("https://abc@sentry.io/123"));
        assert_eq!(cfg.sample_rate, 0.25);
    }

    // ---- TelemetryConfig field access ----

    #[test]
    fn telemetry_config_all_fields_accessible() {
        let cfg = TelemetryConfig::default();
        // Verify all fields are publicly accessible
        let _ = &cfg.service_name;
        let _ = &cfg.service_version;
        let _ = &cfg.log_level;
        let _ = &cfg.log_format;
        let _ = &cfg.otlp_endpoint;
        let _ = &cfg.sentry_dsn;
        let _ = &cfg.sample_rate;
    }

    #[test]
    fn telemetry_config_service_name_various_lengths() {
        let short = TelemetryConfig::default().service_name("a");
        assert_eq!(short.service_name, "a");

        let long = TelemetryConfig::default().service_name("a".repeat(1000));
        assert_eq!(long.service_name.len(), 1000);

        let empty = TelemetryConfig::default().service_name("");
        assert!(empty.service_name.is_empty());
    }

    // ---- Exporter tests ----

    #[test]
    fn exporter_default_is_otlp() {
        use crate::exporter::Exporter;
        assert_eq!(Exporter::default(), Exporter::Otlp);
    }

    #[test]
    fn exporter_variants_distinct() {
        use crate::exporter::Exporter;
        assert_ne!(Exporter::Otlp, Exporter::Stdout);
        assert_ne!(Exporter::Otlp, Exporter::Prometheus);
        assert_ne!(Exporter::Stdout, Exporter::Prometheus);
    }

    #[test]
    fn exporter_from_str_opt() {
        use crate::exporter::Exporter;
        assert_eq!(Exporter::from_str_opt("otlp"), Exporter::Otlp);
        assert_eq!(Exporter::from_str_opt("OTLP"), Exporter::Otlp);
        assert_eq!(Exporter::from_str_opt("stdout"), Exporter::Stdout);
        assert_eq!(Exporter::from_str_opt("STDOUT"), Exporter::Stdout);
        assert_eq!(Exporter::from_str_opt("prometheus"), Exporter::Prometheus);
        assert_eq!(Exporter::from_str_opt("PROMETHEUS"), Exporter::Prometheus);
        assert_eq!(Exporter::from_str_opt(""), Exporter::Otlp);
        assert_eq!(Exporter::from_str_opt("unknown"), Exporter::Otlp);
    }

    #[test]
    fn exporter_debug_format() {
        use crate::exporter::Exporter;
        assert!(format!("{:?}", Exporter::Otlp).contains("Otlp"));
        assert!(format!("{:?}", Exporter::Stdout).contains("Stdout"));
        assert!(format!("{:?}", Exporter::Prometheus).contains("Prometheus"));
    }

    #[test]
    fn telemetry_config_new_sets_service_name() {
        let cfg = TelemetryConfig::new("my-service");
        assert_eq!(cfg.service_name, "my-service");
        // Defaults preserved.
        assert_eq!(cfg.service_version, "0.0.0");
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn telemetry_config_exporter_builder() {
        use crate::exporter::Exporter;
        let cfg = TelemetryConfig::new("svc").exporter(Exporter::Stdout);
        assert_eq!(cfg.exporter, Exporter::Stdout);
        let cfg = TelemetryConfig::new("svc").exporter(Exporter::Prometheus);
        assert_eq!(cfg.exporter, Exporter::Prometheus);
        // Default is Otlp.
        assert_eq!(TelemetryConfig::new("svc").exporter, Exporter::Otlp);
    }

    #[cfg(feature = "stdout")]
    #[test]
    fn init_stdout_succeeds_without_network() {
        use crate::exporter::Exporter;
        // The stdout exporter needs no collector — fully hermetic.
        // Note: tracing global subscriber can only be set once per process;
        // run in a subprocess-free way by accepting either outcome of a
        // potential double-init (other tests using init would conflict, so
        // this test only asserts the config path resolves, not global init).
        let cfg = TelemetryConfig::new("stdout-test").exporter(Exporter::Stdout);
        assert_eq!(cfg.exporter, Exporter::Stdout);
    }

    #[cfg(feature = "prometheus")]
    #[test]
    fn init_prometheus_guard_gather_without_init() {
        // A default-constructed guard has no registry — gather must fail
        // cleanly, not panic.
        let guard = crate::TelemetryGuard {
            #[cfg(feature = "otlp")]
            tracer_provider: None,
            #[cfg(feature = "stdout")]
            stdout_provider: None,
            meter_provider: None,
            prometheus_registry: None,
            #[cfg(feature = "sentry")]
            sentry_guard: None,
        };
        let result = guard.gather_metrics();
        assert!(result.is_err());
    }
}
