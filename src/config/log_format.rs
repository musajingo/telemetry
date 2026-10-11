//! Format of the stdout log lines.

/// Format of the stdout log lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogFormat {
    /// Single-line human-readable output.
    #[default]
    Text,
    /// Multi-line human-readable output, for local development.
    Pretty,
    /// One JSON object per line, for log shippers. Lines emitted inside a
    /// trace carry flat `trace_id` and `span_id` fields.
    #[cfg(feature = "json")]
    Json,
}

impl LogFormat {
    /// Parses `text`, `pretty` or `json`, ignoring case.
    ///
    /// Returns `None` for anything else, and for `json` when the `json`
    /// feature is disabled.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" | "plain" | "full" => Some(Self::Text),
            "pretty" => Some(Self::Pretty),
            #[cfg(feature = "json")]
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

#[cfg(all(test, feature = "json"))]
mod tests {
    use super::*;

    #[test]
    fn json_is_a_known_log_format() {
        assert_eq!(LogFormat::parse("JSON"), Some(LogFormat::Json));
    }
}
