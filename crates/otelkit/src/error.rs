use thiserror::Error;

/// Errors that can occur during telemetry initialization.
#[derive(Debug, Error)]
pub enum TelemetryError {
    /// Failed to connect to the OTLP collector.
    #[error("OTLP connection error: {0}")]
    OtlpConnection(String),

    /// An invalid configuration value was provided.
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}
