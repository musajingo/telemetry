//! Recording the response status on the request span.

use std::time::Duration;

use tower_http::trace::{DefaultOnResponse, OnResponse};
use tracing::Span;

/// A tower-http [`OnResponse`] that records the response status on the request
/// span and flags 5xx responses as span errors, so a tracing backend shows
/// failed requests as failed. The log line itself is delegated to
/// [`DefaultOnResponse`].
#[derive(Clone, Debug, Default)]
pub struct OtelOnResponse {
    inner: DefaultOnResponse,
}

impl OtelOnResponse {
    /// Wraps the `DefaultOnResponse` that writes the response log line.
    pub fn new(inner: DefaultOnResponse) -> Self {
        Self { inner }
    }
}

fn otel_status_for(status: u16) -> Option<&'static str> {
    (status >= 500).then_some("ERROR")
}

impl<B> OnResponse<B> for OtelOnResponse {
    fn on_response(self, response: &http::Response<B>, latency: Duration, span: &Span) {
        let status = response.status().as_u16();
        span.record("http.response.status_code", status);
        if let Some(code) = otel_status_for(status) {
            span.record("otel.status_code", code);
        }
        self.inner.on_response(response, latency, span);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_server_errors_mark_the_span_as_failed() {
        assert_eq!(otel_status_for(200), None);
        assert_eq!(otel_status_for(404), None);
        assert_eq!(otel_status_for(499), None);
        assert_eq!(otel_status_for(500), Some("ERROR"));
        assert_eq!(otel_status_for(503), Some("ERROR"));
    }
}
