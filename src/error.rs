//! The error returned when telemetry cannot be initialized.

use std::fmt;

/// Why [`TelemetryConfig::try_init`](crate::TelemetryConfig::try_init) failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum InitError {
    /// A global `tracing` subscriber is already installed. It can only be set
    /// once per process, so telemetry was already initialized here or by
    /// another library.
    AlreadyInitialized,
    /// The configured log filter is not valid `RUST_LOG` syntax.
    InvalidLogFilter {
        /// The directives that failed to parse.
        directives: String,
        /// The parser's complaint.
        source: tracing_subscriber::filter::ParseError,
    },
    /// The configured trace filter is not valid `RUST_LOG` syntax.
    InvalidTraceFilter {
        /// The directives that failed to parse.
        directives: String,
        /// The parser's complaint.
        source: tracing_subscriber::filter::ParseError,
    },
    /// OTLP over gRPC was configured, but no Tokio runtime is running. The
    /// gRPC client needs one, so initialize inside the runtime (after
    /// `#[tokio::main]` starts, for instance) or use the `otlp-http`
    /// transport, which needs none.
    #[cfg(feature = "otlp-grpc")]
    NoTokioRuntime,
    /// Strict mode: OTLP is selected but no endpoint is configured for any
    /// signal, so nothing would be exported.
    NoExportTarget,
    /// Strict mode: an exporter could not be built.
    ExporterSetup {
        /// Which signal's exporter failed: `traces`, `metrics` or `logs`.
        signal: &'static str,
        /// The exporter builder's complaint.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => {
                f.write_str("a global tracing subscriber is already installed")
            }
            Self::InvalidLogFilter { directives, .. } => {
                write!(f, "invalid log filter {directives:?}")
            }
            Self::InvalidTraceFilter { directives, .. } => {
                write!(f, "invalid trace filter {directives:?}")
            }
            #[cfg(feature = "otlp-grpc")]
            Self::NoTokioRuntime => f.write_str(
                "OTLP over gRPC needs a running Tokio runtime; initialize inside the runtime or use the otlp-http transport",
            ),
            Self::NoExportTarget => f.write_str(
                "strict mode: OTLP export is selected but no endpoint is configured for any signal",
            ),
            Self::ExporterSetup { signal, .. } => {
                write!(f, "strict mode: the {signal} exporter could not be built")
            }
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AlreadyInitialized => None,
            Self::InvalidLogFilter { source, .. } | Self::InvalidTraceFilter { source, .. } => {
                Some(source)
            }
            #[cfg(feature = "otlp-grpc")]
            Self::NoTokioRuntime => None,
            Self::NoExportTarget => None,
            Self::ExporterSetup { source, .. } => Some(source.as_ref()),
        }
    }
}
