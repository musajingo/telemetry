//! A collector address as configured.

/// A collector address as configured, before the protocol's defaults apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Url(String),
    HostPort { host: String, port: Option<String> },
}
