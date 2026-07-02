//! Opt-in OpenTelemetry (OTLP) export — spans AND metrics.
//!
//! Compiled to a real exporter only with the `otlp` feature; without it the
//! function here is a no-op so [`crate`]'s logging setup is byte-identical
//! regardless of the build. Export is OTLP over HTTP/protobuf (reqwest + rustls,
//! already in the tree) rather than gRPC/tonic. See `docs/18-observability.md`.
//!
//! Metrics ride the same `tracing` pipeline: a [`tracing_opentelemetry::MetricsLayer`]
//! turns events carrying `monotonic_counter.*` / `histogram.*` fields into OTLP
//! instruments, so code emits metrics with ordinary `tracing` macros and no crate
//! needs an OpenTelemetry dependency of its own.

use tracing_subscriber::{Layer, Registry};

/// A tracing layer added to the root [`Registry`], boxed so both feature branches
/// present the same type to the subscriber builder.
pub type OtelLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Held for the process lifetime; flushes and shuts the exporters down on drop.
/// A zero-sized no-op when the `otlp` feature is off.
#[cfg(feature = "otlp")]
pub struct OtelGuard {
    tracer: opentelemetry_sdk::trace::SdkTracerProvider,
    meter: opentelemetry_sdk::metrics::SdkMeterProvider,
}

/// No-op guard when the exporter is not compiled in.
#[cfg(not(feature = "otlp"))]
pub struct OtelGuard;

#[cfg(feature = "otlp")]
impl Drop for OtelGuard {
    fn drop(&mut self) {
        // Best-effort flush of any buffered spans + metrics on shutdown.
        let _ = self.tracer.shutdown();
        let _ = self.meter.shutdown();
    }
}

/// Build the OTLP tracing + metrics layers exporting to `endpoint`, plus a guard
/// that flushes on drop. `None` when an exporter cannot be built.
#[cfg(feature = "otlp")]
pub fn otlp_layers(endpoint: &str) -> Option<(Vec<OtelLayer>, OtelGuard)> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;

    // Canonical OpenTelemetry resource keys as plain literals — avoids coupling to
    // the semantic-conventions crate (whose `service.instance.id` sits behind an
    // experimental feature gate); these strings are the stable convention.
    let resource = Resource::builder()
        .with_service_name("cellarr")
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .with_attribute(KeyValue::new(
            "service.instance.id",
            uuid::Uuid::new_v4().to_string(),
        ))
        .build();

    // --- Traces ---
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| eprintln!("warning: OTLP span exporter init failed: {e}"))
        .ok()?;
    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();
    let tracer = tracer_provider.tracer("cellarr");
    let trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // --- Metrics ---
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| eprintln!("warning: OTLP metric exporter init failed: {e}"))
        .ok()?;
    let reader = PeriodicReader::builder(metric_exporter).build();
    let meter_provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource)
        .build();
    let metrics_layer = tracing_opentelemetry::MetricsLayer::new(meter_provider.clone());

    let layers: Vec<OtelLayer> = vec![Box::new(trace_layer), Box::new(metrics_layer)];
    Some((
        layers,
        OtelGuard {
            tracer: tracer_provider,
            meter: meter_provider,
        },
    ))
}

/// No-op stub: without the `otlp` feature there is no exporter to build, so an
/// endpoint (if configured) is ignored and logging stays entirely local.
#[cfg(not(feature = "otlp"))]
pub fn otlp_layers(_endpoint: &str) -> Option<(Vec<OtelLayer>, OtelGuard)> {
    eprintln!(
        "warning: CELLARR_OTEL__ENDPOINT is set but this binary was built without \
         the `otlp` feature; nothing will be exported."
    );
    None
}
