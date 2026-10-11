//! The testing guard is shared across the test binary: no test owns it, so
//! no test can shut telemetry down for the others.

#![cfg(feature = "stdout")]

use telemetry::init_testing_telemetry;
use telemetry::opentelemetry::trace::TraceContextExt as _;
use telemetry::tracing_opentelemetry::OpenTelemetrySpanExt as _;

#[test]
fn the_testing_guard_is_shared_and_retained() {
    let first = init_testing_telemetry("svc");
    // A second call - another test, any name - returns the same guard
    // instead of failing with AlreadyInitialized.
    let second = init_testing_telemetry("other");
    assert!(std::ptr::eq(first, second), "one guard per test binary");
    assert!(first.exports_traces());
}

#[test]
fn telemetry_survives_a_test_dropping_its_binding() {
    {
        let _guard = init_testing_telemetry("svc");
    } // the binding is gone, the shared guard is not

    // Not just enabled by the subscriber: the SDK span behind it is still
    // recording, which a shut-down provider would fail.
    let span = tracing::info_span!("still_alive");
    let context = span.context();
    assert!(context.span().is_recording(), "the SDK is still recording");
}
