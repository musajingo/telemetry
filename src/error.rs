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
        }
    }
}
