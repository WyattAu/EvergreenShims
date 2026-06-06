use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::TracerProvider as SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Initialize the OpenTelemetry tracing pipeline alongside structured logging.
///
/// Spans and events are exported to the given OTLP `endpoint` (e.g.
/// `"http://localhost:4317"`).  The returned [`SdkTracerProvider`] should be
/// shut down when the process exits so that any buffered telemetry is flushed.
pub fn init_otel_tracing(endpoint: &str, debug: bool, json: bool) -> SdkTracerProvider {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .expect("failed to build OTLP span exporter");

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(Resource::new(vec![KeyValue::new(
            "service.name",
            "evergreen-shim",
        )]))
        .build();

    let default_level = if debug { "debug" } else { "info" };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    if json {
        let otel_layer = OpenTelemetryLayer::new(provider.tracer("evergreen-shim"));
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                fmt::layer()
                    .json()
                    .with_target(true)
                    .with_thread_ids(true)
                    .with_file(true)
                    .with_line_number(true),
            )
            .with(otel_layer)
            .init();
    } else {
        let otel_layer = OpenTelemetryLayer::new(provider.tracer("evergreen-shim"));
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer())
            .with(otel_layer)
            .init();
    }

    provider
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_type_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SdkTracerProvider>();
    }
}
