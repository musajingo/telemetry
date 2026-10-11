//! HTTP request tracing that continues an upstream distributed trace.

use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::global;
use opentelemetry_http::HeaderExtractor;
use tower_http::trace::{DefaultOnResponse, MakeSpan, OnResponse};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// A tower-http [`MakeSpan`] that can continue an upstream W3C trace.
///
/// When inbound trace context is trusted (see
/// [`TelemetryConfig::trust_incoming_trace`](crate::TelemetryConfig::trust_incoming_trace)),
/// it extracts the incoming `traceparent`/`tracestate` through the globally
/// registered propagator and attaches the result as the parent of the request
/// span. A caller that already has a trace, such as a frontend rendering on
/// the server or another service, then shares one distributed trace with this
/// service. With no incoming context the request starts a fresh root trace.
///
/// Headers are never recorded on the span: they carry `Authorization` tokens
/// and session cookies, which must not reach stdout logs or the trace store.
#[derive(Clone, Copy, Debug, Default)]
pub struct OtelMakeSpan;

static TRUST_INCOMING_TRACE: OnceLock<bool> = OnceLock::new();

/// Records the configured trust setting. The first call wins, so this is a
/// no-op once a request has already been served.
pub(crate) fn set_trust_incoming_trace(trust: bool) {
    let _ = TRUST_INCOMING_TRACE.set(trust);
}

/// Whether to honour an inbound `traceparent`. Falls back to
/// `TELEMETRY_TRUST_INCOMING_TRACE` when telemetry was never initialized, and
/// to `false` when that is unset.
fn trust_incoming_trace() -> bool {
    *TRUST_INCOMING_TRACE.get_or_init(|| {
        std::env::var("TELEMETRY_TRUST_INCOMING_TRACE")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

impl<B> MakeSpan<B> for OtelMakeSpan {
    fn make_span(&mut self, request: &http::Request<B>) -> Span {
        // The query string is deliberately not recorded: it is where emails,
        // tokens and signatures end up, and those must not reach stdout logs
        // or the trace store.
        let span = tracing::info_span!(
            "request",
            method = %request.method(),
            path = %request.uri().path(),
            version = ?request.version(),
            http.response.status_code = tracing::field::Empty,
            otel.status_code = tracing::field::Empty,
        );
        if trust_incoming_trace() {
            let parent = global::get_text_map_propagator(|propagator| {
                propagator.extract(&HeaderExtractor(request.headers()))
            });
            // Best-effort: a malformed or absent upstream context just leaves
            // this span a root.
            let _ = span.set_parent(parent);
        }
        span
    }
}

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

pub(crate) fn otel_status_for(status: u16) -> Option<&'static str> {
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
