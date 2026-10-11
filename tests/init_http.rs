//! The global init path over HTTP. Deliberately not a Tokio test: the HTTP
//! transport must work without an async runtime.

#![cfg(feature = "otlp-http")]

use std::time::Duration;

use telemetry::{Protocol, TelemetryConfig, current_trace_headers};

/// A local port with nothing listening on it.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener.local_addr().expect("local address").port()
}

#[test]
fn http_export_needs_no_async_runtime() {
    let guard = TelemetryConfig::new("svc")
        .otlp_protocol(Protocol::HttpProtobuf)
        .otlp_endpoint(format!("http://127.0.0.1:{}", closed_port()))
        .metrics_interval(Duration::from_secs(3600))
        .log_filter("info")
        .try_init()
        .expect("init succeeds outside a runtime");

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

    // Flushing to a dead collector reports the failure instead of panicking.
    guard.force_flush();
    drop(guard);
}
