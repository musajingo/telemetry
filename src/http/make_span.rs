//! The request span, optionally parented to a trusted upstream trace.

use std::sync::OnceLock;

use opentelemetry::global;
use opentelemetry_http::HeaderExtractor;
use tower_http::trace::MakeSpan;
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
pub struct OtelMakeSpan {
    trust_override: Option<bool>,
}

impl OtelMakeSpan {
    /// Trusts (or distrusts) inbound trace context for this layer only,
    /// instead of the configured global setting.
    pub fn trust_incoming_trace(mut self, trust: bool) -> Self {
        self.trust_override = Some(trust);
        self
    }
}

static TRUST_INCOMING_TRACE: OnceLock<bool> = OnceLock::new();

/// Records the configured trust setting. The first init wins.
pub(crate) fn set_trust_incoming_trace(trust: bool) {
    let _ = TRUST_INCOMING_TRACE.set(trust);
}

/// Whether to honour an inbound `traceparent`. False before telemetry is
/// initialized, without pinning the lock: a request served that early must
/// not stop init from applying the configured setting.
fn trust_incoming_trace() -> bool {
    TRUST_INCOMING_TRACE.get().copied().unwrap_or(false)
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
        if self.trust_override.unwrap_or_else(trust_incoming_trace) {
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

#[cfg(test)]
mod tests {
    use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_opentelemetry::OpenTelemetryLayer;
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;

    #[test]
    fn requests_before_init_do_not_pin_the_trust_setting() {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let provider = SdkTracerProvider::builder().build();
        let subscriber =
            tracing_subscriber::registry().with(OpenTelemetryLayer::new(provider.tracer("svc")));
        let _default = tracing::subscriber::set_default(subscriber);

        let request = || {
            http::Request::builder()
                .uri("/orders")
                .header(
                    "traceparent",
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                )
                .body(())
                .expect("a request")
        };
        let trace_id_of =
            |span: &tracing::Span| span.context().span().span_context().trace_id().to_string();

        // Before init nothing is trusted: a fresh root trace...
        let span = OtelMakeSpan::default().make_span(&request());
        assert_ne!(
            trace_id_of(&span),
            "4bf92f3577b34da6a3ce929d0e0e4736",
            "untrusted before init"
        );

        // ...and that early request must not pin the setting: trust
        // configured at init still applies.
        set_trust_incoming_trace(true);
        let span = OtelMakeSpan::default().make_span(&request());
        assert_eq!(
            trace_id_of(&span),
            "4bf92f3577b34da6a3ce929d0e0e4736",
            "the configured trust applies after init"
        );
    }

    #[test]
    fn a_per_layer_trust_override_continues_the_inbound_trace() {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let provider = SdkTracerProvider::builder().build();
        let subscriber =
            tracing_subscriber::registry().with(OpenTelemetryLayer::new(provider.tracer("svc")));
        let _default = tracing::subscriber::set_default(subscriber);

        let request = http::Request::builder()
            .uri("/orders")
            .header(
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            )
            .body(())
            .expect("a request");

        let span = OtelMakeSpan::default()
            .trust_incoming_trace(true)
            .make_span(&request);
        let trace_id = span.context().span().span_context().trace_id();
        assert_eq!(trace_id.to_string(), "0af7651916cd43dd8448eb211c80319c");
    }
}
