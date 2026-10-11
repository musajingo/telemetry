//! The three OTLP signals and where each one's exporter sends to.

use super::protocol::Protocol;

/// One of the three OTLP signals, where they need telling apart.
// With no exporter feature the OTLP provider builders have no caller, so
// nothing constructs a Signal.
#[cfg_attr(
    not(any(feature = "otlp-grpc", feature = "otlp-http")),
    allow(dead_code)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Traces,
    Metrics,
    Logs,
}

/// Where one signal's exporter sends to.
#[cfg_attr(
    not(any(feature = "otlp-grpc", feature = "otlp-http")),
    allow(dead_code)
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SignalTarget {
    pub(crate) protocol: Protocol,
    pub(crate) endpoint: String,
    /// Whether the endpoint was configured for this signal specifically. The
    /// OTLP spec uses such an endpoint exactly as given, where the base
    /// endpoint gets `/v1/<signal>` appended over HTTP.
    pub(crate) exact: bool,
}

/// Per-signal overrides of the base endpoint and protocol, as the
/// `OTEL_EXPORTER_OTLP_<SIGNAL>_*` variables allow.
#[derive(Clone, Debug, Default)]
pub(crate) struct SignalOverride {
    pub(crate) endpoint: Option<String>,
    pub(crate) protocol: Option<Protocol>,
}
