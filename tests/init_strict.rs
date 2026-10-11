//! Strict initialization and the guard's view of what is exported. Over HTTP
//! so no Tokio runtime is involved.

#![cfg(feature = "otlp-http")]

use telemetry::{InitError, Protocol, TelemetryConfig};

/// A base URL the exporter builder rejects.
const MALFORMED: &str = "http://exa mple.com";

#[test]
fn strict_init_demands_a_working_export_setup() {
    // Strict with no endpoint configured for any signal: refused, because
    // nothing would be exported.
    let err = TelemetryConfig::new("svc")
        .strict(true)
        .try_init()
        .err()
        .expect("strict init fails with nothing to export");
    assert!(matches!(err, InitError::NoExportTarget), "{err}");

    // Strict with an endpoint the exporter builder rejects: refused, naming
    // the failed exporter.
    let err = TelemetryConfig::new("svc")
        .otlp_protocol(Protocol::HttpProtobuf)
        .otlp_endpoint(MALFORMED)
        .strict(true)
        .try_init()
        .err()
        .expect("strict init fails on a broken exporter");
    assert!(matches!(err, InitError::ExporterSetup { .. }), "{err}");

    // Both refusals left the globals untouched, so a lenient init with the
    // same broken endpoint still boots - and the guard says what happened:
    // nothing is exported, and every exporter failure is reported.
    let guard = TelemetryConfig::new("svc")
        .otlp_protocol(Protocol::HttpProtobuf)
        .otlp_endpoint(MALFORMED)
        .try_init()
        .expect("lenient init boots anyway");
    assert!(!guard.exports_traces());
    assert!(!guard.exports_metrics());
    assert!(!guard.exports_logs());
    assert_eq!(guard.exporter_failures().len(), 3, "one failure per signal");
    assert!(
        guard.exporter_failures()[0].contains("traces"),
        "{:?}",
        guard.exporter_failures()
    );
}
