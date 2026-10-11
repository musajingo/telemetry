//! Stdout-exporting telemetry initialization for tests.

use std::sync::OnceLock;
use std::time::Duration;

use crate::config::{Exporter, LogFormat, TelemetryConfig};
use crate::guard::OtelGuard;

/// Initializes a test-friendly telemetry setup that prints to stdout, once
/// per test binary, and returns the shared guard.
///
/// Every span is sampled and printed as it ends, metrics are printed every
/// five seconds, and log lines are pretty-printed from `WARN` up. The log
/// filter does not touch the spans. Reads no environment variables.
///
/// The guard is kept for the binary's whole life and every call returns it
/// (the first caller's service name wins), so tests can call this in any
/// order and no test can shut telemetry down for the others.
///
/// # Example
///
/// ```no_run
/// #[tokio::test]
/// async fn my_test() {
///     telemetry::init_testing_telemetry("my-service");
///     // Spans and metrics produced from here on are printed to stdout.
/// }
/// ```
///
/// # Panics
///
/// Panics on the first call if a global `tracing` subscriber was already
/// installed by something else.
pub fn init_testing_telemetry(service_name: impl Into<String>) -> &'static OtelGuard {
    static GUARD: OnceLock<OtelGuard> = OnceLock::new();
    GUARD.get_or_init(|| {
        TelemetryConfig::new(service_name)
            .exporter(Exporter::Stdout)
            .sample_ratio(1.0)
            .metrics_interval(Duration::from_secs(5))
            .log_filter("warn")
            .trace_filter("trace")
            .log_format(LogFormat::Pretty)
            .panic_hook(false)
            .init()
    })
}
