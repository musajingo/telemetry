//! The global init path over gRPC. Everything lives in one test because the
//! `tracing` subscriber can only be installed once per process.

#![cfg(feature = "otlp-grpc")]

use std::time::Duration;

use telemetry::{InitError, TelemetryConfig, current_trace_headers};

/// A local port with nothing listening on it.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener.local_addr().expect("local address").port()
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_initializes_once_per_process() {
    // A bad filter fails before the global subscriber is claimed...
    let err = TelemetryConfig::new("svc")
        .log_filter("app=not-a-level")
        .try_init()
        .err()
        .expect("an invalid filter is an error");
    assert!(matches!(err, InitError::InvalidLogFilter { .. }), "{err}");

    // ...so this init still succeeds. No collector is listening, and that
    // must not get in the way of booting.
    let guard = TelemetryConfig::new("svc")
        .service_version("1.2.3")
        .otlp_endpoint(format!("http://127.0.0.1:{}", closed_port()))
        .metrics_interval(Duration::from_secs(3600))
        .log_filter("info")
        .try_init()
        .expect("first init succeeds");

    {
        let span = tracing::info_span!("outbound");
        let _enter = span.enter();
        tracing::info!("inside a span");

        let headers = current_trace_headers();
        assert!(
            headers.iter().any(|(key, _)| key == "traceparent"),
            "the tracer and the W3C propagator are installed: {headers:?}"
        );
    }

    // Both W3C propagators are installed by default, so baggage set upstream
    // is carried through and out of this service.
    let fields: Vec<String> = telemetry::opentelemetry::global::get_text_map_propagator(
        |propagator| propagator.fields().map(String::from).collect(),
    );
    assert!(fields.contains(&"baggage".to_string()), "{fields:?}");

    let err = TelemetryConfig::new("svc")
        .try_init()
        .err()
        .expect("a second init is an error");
    assert!(matches!(err, InitError::AlreadyInitialized), "{err}");

    // Flushing to a dead collector reports the failure instead of panicking.
    guard.force_flush();
    guard.report_exit(&Err::<(), _>("boom"));
    drop(guard);
}
