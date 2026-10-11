//! The pinned OpenTelemetry crates are re-exported so an application writes
//! `telemetry::opentelemetry` and cannot drift to a version whose global
//! providers this crate's layers would not connect to.

use telemetry::opentelemetry::{KeyValue, global};
use telemetry::tracing_opentelemetry::OpenTelemetrySpanExt as _;

#[test]
fn reexported_crates_are_the_ones_the_crate_is_built_against() {
    // `opentelemetry`: the API crate, here recording a metric.
    let meter = global::meter("reexport-test");
    let counter = meter.u64_counter("reexport_test").build();
    counter.add(1, &[KeyValue::new("key", "value")]);

    // `opentelemetry_sdk`: a `Resource` built from the re-export is accepted
    // by an SDK provider builder from the same re-export.
    let resource = telemetry::opentelemetry_sdk::Resource::builder_empty().build();
    let _provider = telemetry::opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_resource(resource)
        .build();

    // `tracing_opentelemetry`: the span extension trait resolves against the
    // re-exported API crate's `Context`.
    let _context: telemetry::opentelemetry::Context = tracing::Span::current().context();
}
