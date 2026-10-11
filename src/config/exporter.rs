//! Where telemetry is sent.

/// Where traces, metrics and log records are sent.
///
/// Stdout logging is independent of this choice and always stays on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Exporter {
    /// OTLP to the configured endpoint, over the configured
    /// [`Protocol`](crate::Protocol). When no endpoint is configured nothing
    /// is exported, so a service boots the same way with or without a
    /// collector.
    #[default]
    Otlp,
    /// Print spans and metrics to stdout. Meant for tests and local runs
    /// where no collector is available.
    #[cfg(feature = "stdout")]
    Stdout,
    /// Export nothing.
    None,
}
