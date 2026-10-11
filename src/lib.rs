// The README doubles as the crate documentation, which also makes every Rust
// example in it a doc test. Its examples need the default feature set.
#![cfg_attr(
    all(feature = "otlp-grpc", feature = "tower-http", feature = "json"),
    doc = include_str!("../README.md")
)]

mod config;
mod error;
mod guard;
#[cfg(feature = "tower-http")]
mod http;
mod init;
mod lifecycle_error;
mod panic_hook;
mod propagate;
mod providers;
#[cfg(feature = "stdout")]
mod testing;

// The OpenTelemetry globals only connect code compiled against the same
// `opentelemetry` version, so the pinned crates are re-exported: depend on
// `telemetry::opentelemetry` instead of declaring your own copy and the
// versions cannot drift apart.
pub use {opentelemetry, opentelemetry_sdk, tracing_opentelemetry};

pub use config::{Exporter, LogFormat, Protocol, TelemetryConfig};
pub use error::InitError;
pub use guard::TelemetryGuard;
#[cfg(feature = "tower-http")]
pub use http::{OtelMakeSpan, OtelOnResponse, http_trace_layer};
pub use init::{init, try_init};
pub use lifecycle_error::LifecycleError;
pub use propagate::current_trace_headers;
#[cfg(feature = "stdout")]
pub use testing::init_testing_telemetry;
