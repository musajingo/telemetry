//! The OTLP transport.

/// The OTLP transport. Each variant exists only when its feature is enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Protocol {
    /// gRPC, conventionally on port `4317`. Needs a Tokio runtime.
    #[cfg(feature = "otlp-grpc")]
    Grpc,
    /// HTTP with protobuf payloads, conventionally on port `4318`. Runs on
    /// the exporters' own threads, so it needs no async runtime.
    #[cfg(feature = "otlp-http")]
    HttpProtobuf,
}

impl Protocol {
    /// Parses the values of the standard `OTEL_EXPORTER_OTLP_PROTOCOL`
    /// variable: `grpc` or `http/protobuf`.
    ///
    /// Returns `None` for anything else (`http/json` is not supported), and
    /// for a protocol whose feature is disabled.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            #[cfg(feature = "otlp-grpc")]
            "grpc" => Some(Self::Grpc),
            #[cfg(feature = "otlp-http")]
            "http/protobuf" => Some(Self::HttpProtobuf),
            _ => None,
        }
    }

    /// gRPC when it is compiled in, otherwise HTTP, otherwise nothing.
    pub(crate) fn preferred() -> Option<Self> {
        #[cfg(feature = "otlp-grpc")]
        {
            Some(Self::Grpc)
        }
        #[cfg(all(not(feature = "otlp-grpc"), feature = "otlp-http"))]
        {
            Some(Self::HttpProtobuf)
        }
        #[cfg(not(any(feature = "otlp-grpc", feature = "otlp-http")))]
        {
            None
        }
    }

    pub(crate) fn default_port(self) -> &'static str {
        match self {
            #[cfg(feature = "otlp-grpc")]
            Self::Grpc => "4317",
            #[cfg(feature = "otlp-http")]
            Self::HttpProtobuf => "4318",
        }
    }
}
