//! The ready-made tower-http trace layer.

use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer};

/// The assembled trace layer for the common case: an [`OtelMakeSpan`] request
/// span and an [`OtelOnResponse`] that records the status and logs the
/// response at `INFO`.
///
/// Plugs into axum, tonic and any other tower service. Customize by
/// replacing parts, e.g. to trust inbound trace context on this layer only:
///
/// ```rust
/// use telemetry::{OtelMakeSpan, http_trace_layer};
///
/// let layer = http_trace_layer()
///     .make_span_with(OtelMakeSpan::default().trust_incoming_trace(true));
/// # let _ = layer;
/// ```
///
/// [`OtelMakeSpan`]: crate::OtelMakeSpan
/// [`OtelOnResponse`]: crate::OtelOnResponse
pub fn http_trace_layer() -> TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    super::OtelMakeSpan,
    DefaultOnRequest,
    super::OtelOnResponse,
> {
    TraceLayer::new_for_http()
        .make_span_with(super::OtelMakeSpan::default())
        .on_response(super::OtelOnResponse::new(
            DefaultOnResponse::new().level(tracing::Level::INFO),
        ))
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tower::ServiceExt as _;
    use tracing_opentelemetry::OpenTelemetryLayer;
    use tracing_subscriber::layer::SubscriberExt as _;

    use crate::http_trace_layer;

    #[tokio::test]
    async fn the_ready_made_layer_produces_request_spans() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber =
            tracing_subscriber::registry().with(OpenTelemetryLayer::new(provider.tracer("svc")));
        let _default = tracing::subscriber::set_default(subscriber);

        let service = tower::ServiceBuilder::new()
            .layer(http_trace_layer())
            .service(tower::service_fn(|_req: http::Request<String>| async {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(500)
                        .body(String::new())
                        .expect("a response"),
                )
            }));

        let request = http::Request::builder()
            .uri("/checkout?token=secret")
            .body(String::new())
            .expect("a request");
        let response = service.oneshot(request).await.expect("a response");
        assert_eq!(response.status(), 500);
        // The request span lives inside the response body wrapper.
        drop(response);

        let spans = exporter.get_finished_spans().expect("finished spans");
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "request");
        assert!(
            span.attributes
                .iter()
                .any(|kv| kv.key.as_str() == "http.response.status_code"
                    && kv.value.to_string() == "500"),
            "{:?}",
            span.attributes
        );
        // The query string is never recorded.
        assert!(
            !format!("{:?}", span.attributes).contains("secret"),
            "{:?}",
            span.attributes
        );
    }
}
