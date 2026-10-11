//! Telemetry initialization: the global subscriber, its layers and the
//! OpenTelemetry globals.

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use tracing::level_filters::LevelFilter;
use tracing_opentelemetry::{MetricsLayer, OpenTelemetryLayer};
use tracing_subscriber::{
    EnvFilter, Layer, filter::Targets, layer::SubscriberExt, registry::LookupSpan,
    util::SubscriberInitExt,
};

use crate::config::{LogFormat, TelemetryConfig};
use crate::error::InitError;
use crate::guard::OtelGuard;
use crate::panic_hook::install_panic_hook;
use crate::providers::Providers;

/// Initializes telemetry from the environment and returns the guard that owns
/// the providers. Keep the guard alive for as long as the process runs.
///
/// Shorthand for `TelemetryConfig::from_env(service_name).init()`.
///
/// # Panics
///
/// Panics where [`try_init_telemetry`] would return an error.
pub fn init_telemetry(service_name: impl Into<String>) -> OtelGuard {
    TelemetryConfig::from_env(service_name).init()
}

/// Initializes telemetry from the environment and returns the guard that owns
/// the providers. Keep the guard alive for as long as the process runs.
///
/// Shorthand for `TelemetryConfig::from_env(service_name).try_init()`.
///
/// # Errors
///
/// See [`TelemetryConfig::try_init`].
pub fn try_init_telemetry(service_name: impl Into<String>) -> Result<OtelGuard, InitError> {
    TelemetryConfig::from_env(service_name).try_init()
}

pub(crate) fn try_init(config: TelemetryConfig) -> Result<OtelGuard, InitError> {
    let env_filter = env_filter(&config)?;
    let providers = Providers::build(&config);

    subscriber(&config, &providers, env_filter)
        .try_init()
        .map_err(|_| InitError::AlreadyInitialized)?;

    // The OpenTelemetry globals are only claimed once the subscriber is in
    // place, so a failed init leaves them alone.
    global::set_text_map_propagator(crate::propagate::propagator_from_spec(
        std::env::var("OTEL_PROPAGATORS").ok().as_deref(),
    ));
    if let Some(provider) = &providers.tracer {
        global::set_tracer_provider(provider.clone());
    }
    if let Some(provider) = &providers.meter {
        global::set_meter_provider(provider.clone());
    }

    #[cfg(feature = "tower-http")]
    crate::http::set_trust_incoming_trace(config.trust_incoming_trace);

    if config.panic_hook {
        install_panic_hook();
    }

    Ok(OtelGuard {
        service: config.service_name,
        tracer_provider: providers.tracer,
        meter_provider: providers.meter,
        logger_provider: providers.logger,
    })
}

/// Stacks the stdout log layer and one layer per active provider on a registry.
fn subscriber(
    config: &TelemetryConfig,
    providers: &Providers,
    env_filter: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync + 'static {
    let otel_layer = providers
        .tracer
        .as_ref()
        .map(|provider| OpenTelemetryLayer::new(provider.tracer(config.service_name.clone())));
    let metrics_layer = providers
        .meter
        .as_ref()
        .map(|provider| MetricsLayer::new(provider.clone()));

    // The log exporter's own network stack logs through tracing. Without this
    // filter those events re-enter the bridge and feed back on themselves.
    let log_bridge = providers.logger.as_ref().map(|provider| {
        OpenTelemetryTracingBridge::new(provider).with_filter(
            Targets::new()
                .with_default(LevelFilter::TRACE)
                .with_target("opentelemetry", LevelFilter::OFF)
                .with_target("tonic", LevelFilter::OFF)
                .with_target("hyper", LevelFilter::OFF)
                .with_target("h2", LevelFilter::OFF)
                .with_target("tower", LevelFilter::OFF),
        )
    });

    // The env filter is global, so it also keeps the exporters' own events
    // away from the OpenTelemetry layers unless they are asked for.
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer(config.log_format))
        .with(metrics_layer)
        .with(otel_layer)
        .with(log_bridge)
}

/// The configured filter, else `RUST_LOG`, else `info`.
fn env_filter(config: &TelemetryConfig) -> Result<EnvFilter, InitError> {
    match &config.log_filter {
        Some(directives) => {
            EnvFilter::try_new(directives).map_err(|source| InitError::InvalidLogFilter {
                directives: directives.clone(),
                source,
            })
        }
        None => Ok(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))),
    }
}

fn fmt_layer<S>(format: LogFormat) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fmt_layer_with(format, std::io::stdout)
}

fn fmt_layer_with<S, W>(format: LogFormat, make_writer: W) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    match format {
        LogFormat::Text => tracing_subscriber::fmt::layer()
            .with_writer(make_writer)
            .boxed(),
        LogFormat::Pretty => tracing_subscriber::fmt::layer()
            .with_writer(make_writer)
            .pretty()
            .boxed(),
        #[cfg(feature = "json")]
        LogFormat::Json => json_layer(make_writer),
    }
}

/// One JSON object per line, in `tracing-subscriber`'s shape, plus flat
/// `trace_id`/`span_id` fields so a log line emitted inside a trace can be
/// joined to that trace by a log pipeline. The ids come from the span
/// extensions the OpenTelemetry layer maintains, so without a tracer (no
/// exporter configured) they are simply omitted.
#[cfg(feature = "json")]
fn json_layer<S, W>(make_writer: W) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    use std::sync::{Arc, OnceLock};

    use opentelemetry::trace::TraceContextExt as _;
    use tracing::dispatcher::WeakDispatch;
    use tracing::span::Id;

    /// Hands the mappers below the installed `Dispatch`, which
    /// `get_otel_context` needs. `dispatcher::get_default` cannot provide it:
    /// the mappers run while an event is being dispatched, where it returns
    /// the no-op dispatcher to prevent reentry.
    struct CaptureDispatch(Arc<OnceLock<WeakDispatch>>);

    impl<S: tracing::Subscriber> Layer<S> for CaptureDispatch {
        fn on_register_dispatch(&self, dispatch: &tracing::Dispatch) {
            let _ = self.0.set(dispatch.downgrade());
        }
    }

    fn span_context_part(
        dispatch: &OnceLock<WeakDispatch>,
        id: &Id,
        part: impl Fn(&opentelemetry::trace::SpanContext) -> String,
    ) -> Option<String> {
        let dispatch = dispatch.get()?.upgrade()?;
        let context = tracing_opentelemetry::get_otel_context(id, &dispatch)?;
        let span = context.span();
        let span_context = span.span_context();
        span_context.is_valid().then(|| part(span_context))
    }

    let dispatch: Arc<OnceLock<WeakDispatch>> = Arc::default();

    let mut layer = json_subscriber::JsonLayer::new(make_writer);
    layer.with_timer("timestamp", tracing_subscriber::fmt::time::SystemTime);
    layer.with_level("level");
    layer.with_event("fields");
    layer.with_target("target");
    layer.with_current_span("span");
    layer.with_span_list("spans");
    layer.add_from_span("trace_id", {
        let dispatch = dispatch.clone();
        move |span| {
            span_context_part(&dispatch, &span.id(), |span_context| {
                span_context.trace_id().to_string()
            })
        }
    });
    layer.add_from_span("span_id", {
        let dispatch = dispatch.clone();
        move |span| {
            span_context_part(&dispatch, &span.id(), |span_context| {
                span_context.span_id().to_string()
            })
        }
    });

    CaptureDispatch(dispatch).and_then(layer).boxed()
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::TraceContextExt as _;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;

    use super::*;

    #[test]
    fn log_records_carry_the_trace_id_of_the_span_they_were_emitted_in() {
        let spans = InMemorySpanExporter::default();
        let logs = InMemoryLogExporter::default();
        let providers = Providers {
            tracer: Some(
                SdkTracerProvider::builder()
                    .with_simple_exporter(spans.clone())
                    .build(),
            ),
            meter: None,
            logger: Some(
                SdkLoggerProvider::builder()
                    .with_simple_exporter(logs.clone())
                    .build(),
            ),
        };
        let config = TelemetryConfig::new("svc");
        let subscriber = subscriber(&config, &providers, EnvFilter::new("info"));

        let trace_id = tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("work");
            let _enter = span.enter();
            tracing::info!("inside the span");
            tracing::debug!("filtered out");
            span.context().span().span_context().trace_id()
        });

        let records = logs.get_emitted_logs().expect("emitted logs");
        assert_eq!(records.len(), 1, "the env filter applies to the bridge too");
        let context = records[0]
            .record
            .trace_context()
            .expect("log record has a trace context");
        assert_eq!(context.trace_id, trace_id);

        let finished = spans.get_finished_spans().expect("finished spans");
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].span_context.trace_id(), trace_id);
    }

    #[cfg(feature = "json")]
    mod json_output {
        use std::io;
        use std::sync::{Arc, Mutex};

        use tracing_subscriber::fmt::MakeWriter;

        use super::*;

        /// A `MakeWriter` the test can read back.
        #[derive(Clone, Default)]
        pub(super) struct Buffer(Arc<Mutex<Vec<u8>>>);

        impl Buffer {
            pub(super) fn contents(&self) -> String {
                String::from_utf8(self.0.lock().expect("buffer lock").clone())
                    .expect("utf-8 log output")
            }
        }

        impl io::Write for Buffer {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().expect("buffer lock").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        impl<'a> MakeWriter<'a> for Buffer {
            type Writer = Self;
            fn make_writer(&'a self) -> Self {
                self.clone()
            }
        }

        #[test]
        fn json_log_lines_carry_the_ids_of_the_active_trace() {
            let buffer = Buffer::default();
            let provider = SdkTracerProvider::builder()
                .with_simple_exporter(InMemorySpanExporter::default())
                .build();
            let subscriber = tracing_subscriber::registry()
                .with(fmt_layer_with(LogFormat::Json, buffer.clone()))
                .with(OpenTelemetryLayer::new(provider.tracer("svc")));

            let trace_id = tracing::subscriber::with_default(subscriber, || {
                let span = tracing::info_span!("work");
                let _enter = span.enter();
                tracing::info!("inside the span");
                span.context().span().span_context().trace_id().to_string()
            });

            let output = buffer.contents();
            let line = output
                .lines()
                .find(|line| line.contains("inside the span"))
                .expect("the event was printed");
            let json: serde_json::Value =
                serde_json::from_str(line).expect("one JSON object per line");

            // The ids a log shipper joins on, flat at the top level.
            assert_eq!(json["trace_id"].as_str(), Some(trace_id.as_str()), "{json}");
            assert_eq!(json["span_id"].as_str().map(str::len), Some(16), "{json}");

            // The usual shape of a JSON log line is intact.
            assert_eq!(json["fields"]["message"].as_str(), Some("inside the span"));
            assert_eq!(json["level"].as_str(), Some("INFO"));
            assert!(json["timestamp"].is_string(), "{json}");
            assert_eq!(json["span"]["name"].as_str(), Some("work"));
        }

        #[test]
        fn json_log_lines_omit_the_ids_without_a_tracer() {
            let buffer = Buffer::default();
            let subscriber =
                tracing_subscriber::registry().with(fmt_layer_with(LogFormat::Json, buffer.clone()));

            tracing::subscriber::with_default(subscriber, || {
                let span = tracing::info_span!("work");
                let _enter = span.enter();
                tracing::info!("no tracer installed");
            });

            let output = buffer.contents();
            let line = output
                .lines()
                .find(|line| line.contains("no tracer installed"))
                .expect("the event was printed");
            let json: serde_json::Value =
                serde_json::from_str(line).expect("one JSON object per line");

            assert!(json.get("trace_id").is_none(), "{json}");
            assert!(json.get("span_id").is_none(), "{json}");
        }
    }

    #[test]
    fn an_invalid_log_filter_is_reported() {
        let config = TelemetryConfig::new("svc").log_filter("app=not-a-level");
        let err = env_filter(&config).expect_err("filter should not parse");
        assert!(matches!(err, InitError::InvalidLogFilter { .. }), "{err}");
        assert!(err.to_string().contains("app=not-a-level"), "{err}");
    }
}
