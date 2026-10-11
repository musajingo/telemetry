//! Telemetry configuration: the config builder and the types it is built
//! from, one per file.

mod endpoint;
mod exporter;
mod log_format;
mod protocol;
mod signal;
mod telemetry_config;

pub use exporter::Exporter;
pub use log_format::LogFormat;
pub use protocol::Protocol;
#[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
pub(crate) use signal::{Signal, SignalTarget};
pub use telemetry_config::TelemetryConfig;
