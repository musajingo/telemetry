//! Outbound trace-context propagation for clients that are not `http` based.

use opentelemetry::global;
use opentelemetry::propagation::Injector;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

struct VecInjector(Vec<(String, String)>);

impl Injector for VecInjector {
    fn set(&mut self, key: &str, value: String) {
        self.0.push((key.to_string(), value));
    }
}

/// The W3C `traceparent`/`tracestate` pairs for the current span, ready to be
/// copied onto an outbound request (gRPC metadata, a queue message and so on)
/// so the callee's spans join this trace. Empty when there is no active trace.
pub fn current_trace_headers() -> Vec<(String, String)> {
    let mut injector = VecInjector(Vec::new());
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&Span::current().context(), &mut injector)
    });
    injector.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_the_active_span_as_a_traceparent() {
        use opentelemetry::trace::TracerProvider as _;
        use tracing_subscriber::layer::SubscriberExt;

        global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::OpenTelemetryLayer::new(provider.tracer("test")),
        );
        let _default = tracing::subscriber::set_default(subscriber);

        let span = tracing::info_span!("outbound");
        let _enter = span.enter();

        let headers = current_trace_headers();
        let traceparent = headers
            .iter()
            .find(|(k, _)| k == "traceparent")
            .map(|(_, v)| v.as_str())
            .expect("traceparent injected");
        assert!(traceparent.starts_with("00-"), "{traceparent}");
        assert!(!traceparent.contains("00000000000000000000000000000000"));
    }
}
