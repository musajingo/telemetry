//! The gRPC transport needs a Tokio runtime. Asking for it outside one must
//! be an error from `try_init`, not a panic from inside the transport.

#![cfg(feature = "otlp-grpc")]

use telemetry::{InitError, TelemetryConfig};

#[test]
fn grpc_outside_a_runtime_is_an_error_not_a_panic() {
    let err = TelemetryConfig::new("svc")
        .otlp_endpoint("http://127.0.0.1:4317")
        .try_init()
        .err()
        .expect("an error, not a panic");
    assert!(matches!(err, InitError::NoTokioRuntime), "{err}");

    // The failed init left every global untouched, so initializing properly
    // afterwards still works.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let guard = runtime.block_on(async {
        TelemetryConfig::new("svc")
            .otlp_endpoint("http://127.0.0.1:4317")
            .try_init()
            .expect("init succeeds inside a runtime")
    });
    drop(guard);
}
