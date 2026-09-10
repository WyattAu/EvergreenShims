//! Telemetry exporter backend selection.
//!
//! [`Exporter`] chooses where spans (and, for Prometheus, metrics) go.
//! The default is [`Exporter::Otlp`], which preserves the historical
//! behavior of [`crate::init`]: OTLP export when an endpoint is configured,
//! plain structured logging otherwise.

/// Backend that receives exported telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Exporter {
    /// OTLP over HTTP (requires the `otlp` feature).
    ///
    /// This is the default and preserves historical behavior: spans are
    /// exported to the configured collector endpoint.
    #[default]
    Otlp,
    /// Standard output (requires the `stdout` feature).
    ///
    /// Spans are printed to stdout in a human-readable debug format. Useful
    /// for local development and for environments without a collector. Needs
    /// no network access and is fully hermetic in tests.
    Stdout,
    /// Prometheus metrics exposition (requires the `prometheus` feature).
    ///
    /// Installs a meter provider backed by an in-process Prometheus
    /// registry. Scrape the exposition text via
    /// [`crate::TelemetryGuard::gather_metrics`]. Tracing output continues
    /// through the configured log format layer.
    Prometheus,
}

impl Exporter {
    /// Parse an exporter name, defaulting to [`Exporter::Otlp`].
    ///
    /// Recognized (case-insensitive): `"otlp"`, `"stdout"`, `"prometheus"`.
    /// Anything else falls back to `Otlp`.
    pub fn from_str_opt(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "stdout" => Exporter::Stdout,
            "prometheus" => Exporter::Prometheus,
            _ => Exporter::Otlp,
        }
    }
}
