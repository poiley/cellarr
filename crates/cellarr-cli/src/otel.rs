//! Opt-in OpenTelemetry (OTLP) span export.
//!
//! Compiled to a real exporter only with the `otlp` feature; without it the
//! function here is a no-op so [`crate`]'s logging setup is byte-identical
//! regardless of the build. Export is OTLP over HTTP/protobuf (reqwest + rustls,
//! already in the tree) rather than gRPC/tonic. See `docs/18-observability.md`.

use tracing_subscriber::{Layer, Registry};

/// A tracing layer added to the root [`Registry`], boxed so both feature
/// branches present the same type to the subscriber builder.
pub type OtelLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Held for the process lifetime; flushes and shuts the exporter down on drop.
/// A zero-sized no-op when the `otlp` feature is off.
#[cfg(feature = "otlp")]
pub struct OtelGuard(opentelemetry_sdk::trace::SdkTracerProvider);

/// No-op guard when the exporter is not compiled in.
#[cfg(not(feature = "otlp"))]
pub struct OtelGuard;

#[cfg(feature = "otlp")]
impl Drop for OtelGuard {
    fn drop(&mut self) {
        // Best-effort flush of any buffered spans on shutdown.
        let _ = self.0.shutdown();
    }
}

/// Build an OTLP tracing layer exporting to `endpoint`, plus a guard that flushes
/// on drop. `None` when the exporter cannot be built.
#[cfg(feature = "otlp")]
pub fn otlp_layer(endpoint: &str) -> Option<(OtelLayer, OtelGuard)> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;
    use opentelemetry_semantic_conventions as semconv;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| eprintln!("warning: OTLP exporter init failed: {e}"))
        .ok()?;

    let resource = Resource::builder()
        .with_service_name("cellarr")
        .with_attribute(KeyValue::new(
            semconv::resource::SERVICE_VERSION,
            env!("CARGO_PKG_VERSION"),
        ))
        .with_attribute(KeyValue::new(
            semconv::resource::SERVICE_INSTANCE_ID,
            uuid::Uuid::new_v4().to_string(),
        ))
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();
    let tracer = provider.tracer("cellarr");
    let layer = tracing_opentelemetry::layer().with_tracer(tracer);
    Some((Box::new(layer), OtelGuard(provider)))
}

/// No-op stub: without the `otlp` feature there is no exporter to build, so an
/// endpoint (if configured) is ignored and logging stays entirely local.
#[cfg(not(feature = "otlp"))]
pub fn otlp_layer(_endpoint: &str) -> Option<(OtelLayer, OtelGuard)> {
    eprintln!(
        "warning: CELLARR_OTEL__ENDPOINT is set but this binary was built without \
         the `otlp` feature; no traces will be exported."
    );
    None
}
