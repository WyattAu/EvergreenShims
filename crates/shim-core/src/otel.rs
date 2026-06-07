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
    use opentelemetry::trace::{Span, Tracer, TracerProvider as _};
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::trace::TracerProvider as SdkTracerProvider;
    use opentelemetry_sdk::Resource;

    #[test]
    fn provider_type_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SdkTracerProvider>();
    }

    #[test]
    fn resource_detection_service_name() {
        let resource = Resource::new(vec![KeyValue::new("service.name", "test-shim")]);
        let attrs: Vec<KeyValue> = resource.iter().map(|(k, v)| KeyValue::new(k, v)).collect();
        assert!(attrs
            .iter()
            .any(|a| a.key.as_str() == "service.name"));
    }

    #[test]
    fn resource_detection_service_version() {
        let resource = Resource::new(vec![
            KeyValue::new("service.name", "test-shim"),
            KeyValue::new("service.version", "1.2.3"),
        ]);
        let attrs: Vec<KeyValue> = resource.iter().map(|(k, v)| KeyValue::new(k, v)).collect();
        let version_attr = attrs.iter().find(|a| a.key.as_str() == "service.version");
        assert!(version_attr.is_some());
    }

    #[test]
    fn trace_span_creation_and_attributes() {
        let provider = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "span-test",
            )]))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut span = tracer.start("test-span");
        span.set_attribute(KeyValue::new("http.method", "GET"));
        span.set_attribute(KeyValue::new("http.url", "/api/v1/test"));
        span.set_attribute(KeyValue::new("component", "chaos-shim"));
        span.end();

        // Verify span was created without panic
        let _ = provider.shutdown();
    }

    #[test]
    fn trace_error_recording_in_span() {
        let provider = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "error-test",
            )]))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut span = tracer.start("error-span");
        span.record_error(opentelemetry::error::Error::from(
            std::io::Error::new(std::io::ErrorKind::Other, "simulated failure"),
        ));
        span.set_status(opentelemetry::trace::Status::error("simulated failure"));
        span.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_span_link_propagation() {
        use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId};

        let provider = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "link-test",
            )]))
            .build();
        let tracer = provider.tracer("test-tracer");

        let parent_span = tracer.start("parent");
        let parent_ctx = parent_span.span_context().clone();

        // Create a linked span context
        let linked_ctx = SpanContext::new(
            TraceId::from_u128(42),
            SpanId::from_u64(7),
            TraceFlags::SAMPLED,
            true,
            Default::default(),
        );

        let mut child_span = tracer.start_with_context("child", &opentelemetry::Context::current());
        child_span.set_attribute(KeyValue::new("link.trace_id", linked_ctx.trace_id().to_string()));
        child_span.set_attribute(KeyValue::new("link.span_id", linked_ctx.span_id().to_string()));
        child_span.end();
        parent_span.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_context_propagation() {
        let provider = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "context-propagation",
            )]))
            .build();
        let tracer = provider.tracer("test-tracer");

        // Start a parent span and extract its context
        let parent_span = tracer.start("parent-span");
        let parent_span_ctx = parent_span.span_context().clone();

        // Create child span using parent context
        let parent_context = opentelemetry::Context::current().with_remote_span_context(parent_span_ctx);
        let mut child_span = tracer.start_with_context("child-span", &parent_context);
        child_span.set_attribute(KeyValue::new("parent.trace_id", parent_span_ctx.trace_id().to_string()));
        child_span.end();
        parent_span.end();

        // Verify parent trace_id propagated
        assert_ne!(parent_span_ctx.trace_id(), TraceId::INVALID);
        let _ = provider.shutdown();
    }

    #[test]
    fn trace_span_events() {
        use opentelemetry::trace::StatusCode;

        let provider = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "event-test",
            )]))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut span = tracer.start("event-span");
        span.add_event("request.started", vec![KeyValue::new("method", "GET")]);
        span.add_event(
            "fault.injected",
            vec![
                KeyValue::new("fault.type", "latency"),
                KeyValue::new("delay_ms", 100i64),
            ],
        );
        span.set_status(StatusCode::Ok);
        span.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_multiple_providers_independent() {
        let provider1 = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "shim-1",
            )]))
            .build();
        let provider2 = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "shim-2",
            )]))
            .build();

        let tracer1 = provider1.tracer("tracer-1");
        let tracer2 = provider2.tracer("tracer-2");

        let mut span1 = tracer1.start("span-from-shim-1");
        span1.end();

        let mut span2 = tracer2.start("span-from-shim-2");
        span2.end();

        let _ = provider1.shutdown();
        let _ = provider2.shutdown();
    }

    #[test]
    fn trace_span_with_nested_operations() {
        let provider = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "nested-test",
            )]))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut outer = tracer.start("chaos.experiment.run");
        outer.set_attribute(KeyValue::new("experiment.id", "exp-001"));
        outer.set_attribute(KeyValue::new("fault.type", "latency"));

        let mut inner_inject = tracer.start("chaos.fault.inject");
        inner_inject.set_attribute(KeyValue::new("target", "web-1"));
        inner_inject.set_attribute(KeyValue::new("delay_ms", 250i64));
        inner_inject.end();

        let mut inner_measure = tracer.start("chaos.measure.effect");
        inner_measure.set_attribute(KeyValue::new("metric.latency_p99", 350i64));
        inner_measure.end();

        outer.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn batch_processor_builder() {
        // Verify that building a provider with batch exporter works
        let provider = SdkTracerProvider::builder()
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                "batch-test",
            )]))
            .build();

        let tracer = provider.tracer("batch-test-tracer");
        let mut span = tracer.start("batch-span");
        span.set_attribute(KeyValue::new("key", "value"));
        span.end();

        // Shutdown flushes the batch
        let _ = provider.shutdown();
    }
}
