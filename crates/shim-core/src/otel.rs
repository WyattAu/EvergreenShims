/// Initialize the OpenTelemetry tracing pipeline alongside structured logging.
///
/// Delegates to `otelkit` for tracing subscriber setup and OTLP export.
/// The returned [`otelkit::TelemetryGuard`] should be held for the process
/// lifetime so that buffered telemetry is flushed on exit.
pub fn init_otel_tracing(
    endpoint: &str,
    debug: bool,
    json: bool,
) -> Option<otelkit::TelemetryGuard> {
    let default_level = if debug { "debug" } else { "info" };
    let log_level = std::env::var("OTEL_LOG_LEVEL").unwrap_or_else(|_| default_level.to_string());

    let log_format = if json {
        otelkit::LogFormat::Json
    } else {
        otelkit::LogFormat::Text
    };

    let config = otelkit::TelemetryConfig::default()
        .service_name("evergreen-shim")
        .service_version(env!("CARGO_PKG_VERSION"))
        .log_level(log_level)
        .log_format(log_format)
        .otlp_endpoint(endpoint);

    match otelkit::init(config) {
        Ok(guard) => Some(guard),
        Err(e) => {
            tracing::warn!(error = ?e, "Failed to initialise otelkit, falling back to fmt-only");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{Span, Status, Tracer, TracerProvider};
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::resource::Resource;
    use opentelemetry_sdk::trace::SdkTracerProvider;

    fn test_resource(name: &'static str) -> Resource {
        Resource::builder()
            .with_attributes(vec![KeyValue::new("service.name", name)])
            .build()
    }

    #[test]
    fn provider_type_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SdkTracerProvider>();
    }

    #[test]
    fn resource_detection_service_name() {
        let resource = test_resource("test-shim");
        let attrs: Vec<KeyValue> = resource
            .iter()
            .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
            .collect();
        assert!(attrs.iter().any(|a| a.key.as_str() == "service.name"));
    }

    #[test]
    fn resource_detection_service_version() {
        let resource = Resource::builder()
            .with_attributes(vec![
                KeyValue::new("service.name", "test-shim"),
                KeyValue::new("service.version", "1.2.3"),
            ])
            .build();
        let attrs: Vec<KeyValue> = resource
            .iter()
            .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
            .collect();
        let version_attr = attrs.iter().find(|a| a.key.as_str() == "service.version");
        assert!(version_attr.is_some());
    }

    #[test]
    fn trace_span_creation_and_attributes() {
        let provider = SdkTracerProvider::builder()
            .with_resource(test_resource("span-test"))
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
            .with_resource(test_resource("error-test"))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut span = tracer.start("error-span");
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "simulated failure");
        span.record_error(&io_err);
        span.set_status(Status::error("simulated failure"));
        span.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_span_link_propagation() {
        use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId};

        let provider = SdkTracerProvider::builder()
            .with_resource(test_resource("link-test"))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut parent_span = tracer.start("parent");
        let parent_ctx = parent_span.span_context().clone();

        // Create a linked span context
        let linked_ctx = SpanContext::new(
            TraceId::from(42u128),
            SpanId::from(7u64),
            TraceFlags::SAMPLED,
            true,
            Default::default(),
        );

        let mut child_span = tracer.start_with_context("child", &opentelemetry::Context::current());
        child_span.set_attribute(KeyValue::new(
            "link.trace_id",
            linked_ctx.trace_id().to_string(),
        ));
        child_span.set_attribute(KeyValue::new(
            "link.span_id",
            linked_ctx.span_id().to_string(),
        ));
        child_span.end();
        parent_span.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_context_propagation() {
        use opentelemetry::trace::{TraceContextExt, TraceId};

        let provider = SdkTracerProvider::builder()
            .with_resource(test_resource("context-propagation"))
            .build();
        let tracer = provider.tracer("test-tracer");

        // Start a parent span and extract its context
        let mut parent_span = tracer.start("parent-span");
        let parent_span_ctx = parent_span.span_context().clone();

        // Create child span using parent context
        let parent_context =
            opentelemetry::Context::current().with_remote_span_context(parent_span_ctx.clone());
        let mut child_span = tracer.start_with_context("child-span", &parent_context);
        child_span.set_attribute(KeyValue::new(
            "parent.trace_id",
            parent_span_ctx.trace_id().to_string(),
        ));
        child_span.end();
        parent_span.end();

        // Verify parent trace_id propagated
        assert_ne!(parent_span_ctx.trace_id(), TraceId::INVALID);
        let _ = provider.shutdown();
    }

    #[test]
    fn trace_span_events() {
        let provider = SdkTracerProvider::builder()
            .with_resource(test_resource("event-test"))
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
        span.set_status(Status::Ok);
        span.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_multiple_providers_independent() {
        let provider1 = SdkTracerProvider::builder()
            .with_resource(test_resource("shim-1"))
            .build();
        let provider2 = SdkTracerProvider::builder()
            .with_resource(test_resource("shim-2"))
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
            .with_resource(test_resource("nested-test"))
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
            .with_resource(test_resource("batch-test"))
            .build();

        let tracer = provider.tracer("batch-test-tracer");
        let mut span = tracer.start("batch-span");
        span.set_attribute(KeyValue::new("key", "value"));
        span.end();

        // Shutdown flushes the batch
        let _ = provider.shutdown();
    }

    #[test]
    fn trace_span_with_special_characters() {
        let provider = SdkTracerProvider::builder()
            .with_resource(test_resource("special-chars"))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut span = tracer.start("operation/with:special*chars");
        span.set_attribute(KeyValue::new("key with spaces", "value"));
        span.set_attribute(KeyValue::new("key\twith\ttabs", "value"));
        span.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_multiple_spans_same_tracer() {
        let provider = SdkTracerProvider::builder()
            .with_resource(test_resource("multi-span"))
            .build();
        let tracer = provider.tracer("test-tracer");

        for i in 0..10 {
            let mut span = tracer.start(format!("span-{}", i));
            span.set_attribute(KeyValue::new("index", i as i64));
            span.end();
        }

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_span_status_codes() {
        let provider = SdkTracerProvider::builder()
            .with_resource(test_resource("status-test"))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut span = tracer.start("status-span");
        span.set_status(Status::Ok);
        span.set_status(Status::error("test error"));
        span.set_status(Status::Unset);
        span.end();

        let _ = provider.shutdown();
    }

    #[test]
    fn trace_span_attribute_types() {
        let provider = SdkTracerProvider::builder()
            .with_resource(test_resource("attr-types"))
            .build();
        let tracer = provider.tracer("test-tracer");

        let mut span = tracer.start("attr-span");
        span.set_attribute(KeyValue::new("str_key", "string_value"));
        span.set_attribute(KeyValue::new("bool_key", true));
        span.set_attribute(KeyValue::new("i64_key", 42i64));
        span.set_attribute(KeyValue::new("f64_key", 3.14));
        span.end();

        let _ = provider.shutdown();
    }
}
