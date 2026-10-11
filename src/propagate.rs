//! The propagators that carry context across service boundaries, and outbound
//! propagation for clients that are not `http` based.

use opentelemetry::global;
use opentelemetry::propagation::{Injector, TextMapCompositePropagator, TextMapPropagator};
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Builds the global propagator from the standard `OTEL_PROPAGATORS` value: a
/// comma-separated list drawn from `tracecontext`, `baggage` and `none`.
///
/// Unset or blank, both W3C propagators are installed, which is the
/// OpenTelemetry default. `none` turns propagation off entirely. Entries this
/// crate does not ship (`b3`, `jaeger`, ...) are reported on stderr and
/// skipped, as the specification asks.
pub(crate) fn propagator_from_spec(spec: Option<&str>) -> TextMapCompositePropagator {
    let spec = spec
        .map(str::trim)
        .filter(|spec| !spec.is_empty())
        .unwrap_or("tracecontext,baggage");

    let mut trace_context = false;
    let mut baggage = false;
    for entry in spec.split(',') {
        match entry.trim().to_ascii_lowercase().as_str() {
            "" => {}
            "tracecontext" => trace_context = true,
            "baggage" => baggage = true,
            "none" => return TextMapCompositePropagator::new(Vec::new()),
            other => eprintln!(
                "telemetry: ignoring unsupported OTEL_PROPAGATORS entry {other:?} (supported: tracecontext, baggage, none)"
            ),
        }
    }

    let mut propagators: Vec<Box<dyn TextMapPropagator + Send + Sync>> = Vec::new();
    if trace_context {
        propagators.push(Box::new(TraceContextPropagator::new()));
    }
    if baggage {
        propagators.push(Box::new(BaggagePropagator::new()));
    }
    TextMapCompositePropagator::new(propagators)
}

struct VecInjector(Vec<(String, String)>);

impl Injector for VecInjector {
    fn set(&mut self, key: &str, value: String) {
        self.0.push((key.to_string(), value));
    }
}

/// The W3C pairs for the current span — `traceparent`/`tracestate` and, when
/// the context carries any, `baggage` — ready to be copied onto an outbound
/// request (gRPC metadata, a queue message and so on) so the callee's spans
/// join this trace. Empty when there is no active trace.
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

    fn fields(propagator: &impl opentelemetry::propagation::TextMapPropagator) -> Vec<String> {
        propagator.fields().map(String::from).collect()
    }

    #[test]
    fn the_default_propagators_are_trace_context_and_baggage() {
        let default = fields(&propagator_from_spec(None));
        assert!(default.contains(&"traceparent".to_string()), "{default:?}");
        assert!(default.contains(&"tracestate".to_string()), "{default:?}");
        assert!(default.contains(&"baggage".to_string()), "{default:?}");

        // Blank means unset, and duplicates collapse.
        assert_eq!(fields(&propagator_from_spec(Some("  "))).len(), 3);
        assert_eq!(
            fields(&propagator_from_spec(Some("baggage,baggage"))),
            vec!["baggage".to_string()]
        );
    }

    #[test]
    fn otel_propagators_selects_the_propagators() {
        let selected = fields(&propagator_from_spec(Some("tracecontext")));
        assert!(selected.contains(&"traceparent".to_string()), "{selected:?}");
        assert!(!selected.contains(&"baggage".to_string()), "{selected:?}");
    }

    #[test]
    fn none_disables_propagation() {
        assert!(fields(&propagator_from_spec(Some("none"))).is_empty());
        assert!(fields(&propagator_from_spec(Some("tracecontext,none"))).is_empty());
    }

    #[test]
    fn unknown_entries_are_skipped_not_fatal() {
        let propagator = propagator_from_spec(Some("b3, baggage"));
        assert_eq!(fields(&propagator), vec!["baggage".to_string()]);
    }

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
