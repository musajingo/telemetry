//! Stdout-exporting telemetry initialization for tests.

use std::time::Duration;

use crate::config::{Exporter, LogFormat, TelemetryConfig};
use crate::guard::OtelGuard;

/// Initializes a test-friendly telemetry setup that prints to stdout.
///
/// For tests and local runs where you want to see spans and metrics without a
/// collector. Every span is sampled and printed as it ends, metrics are
/// printed every five seconds, and log lines are pretty-printed from `WARN`
/// up. The log filter does not touch the spans. Reads no environment
/// variables.
///
/// The subscriber is global, so this can run once per test binary. Use
/// [`TelemetryConfig::try_init`] with [`Exporter::Stdout`] where several tests
/// may race to initialize it.
///
/// # Example
///
/// ```no_run
/// #[tokio::test]
/// async fn my_test() {
///     let _guard = telemetry::init_testing_telemetry("my-service");
///     // Spans and metrics produced from here on are printed to stdout.
/// }
/// ```
///
/// # Panics
///
/// Panics if a global `tracing` subscriber is already installed.
pub fn init_testing_telemetry(service_name: impl Into<String>) -> OtelGuard {
    TelemetryConfig::new(service_name)
        .exporter(Exporter::Stdout)
        .sample_ratio(1.0)
        .metrics_interval(Duration::from_secs(5))
        .log_filter("warn")
        .trace_filter("trace")
        .log_format(LogFormat::Pretty)
        .panic_hook(false)
        .init()
}
