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
    let filters = filters(&config)?;

    // The gRPC exporters capture the current Tokio runtime as they are
    // built, and tonic's machinery panics when there is none. Catch that
    // here, before any global state is touched, and make it a setup error.
    #[cfg(feature = "otlp-grpc")]
    if needs_tokio_runtime(&config) && tokio::runtime::Handle::try_current().is_err() {
        return Err(InitError::NoTokioRuntime);
    }

    let mut providers = Providers::build(&config);

    // Strict mode fails init where the default reports and degrades.
    if config.strict && config.exporter == crate::config::Exporter::Otlp {
        if !providers.failures.is_empty() {
            let (signal, source) = providers.failures.remove(0);
            return Err(InitError::ExporterSetup { signal, source });
        }
        if providers.tracer.is_none() && providers.meter.is_none() && providers.logger.is_none() {
            return Err(InitError::NoExportTarget);
        }
    }
    let exporter_failures: Vec<String> = providers
        .failures
        .drain(..)
        .map(|(signal, err)| {
            let message = format!("OTLP {signal} exporter disabled ({err})");
            eprintln!("telemetry: {message}");
            message
        })
        .collect();

    subscriber(&config, &providers, filters)
        .try_init()
        .map_err(|_| InitError::AlreadyInitialized)?;

    // The OpenTelemetry globals are only claimed once the subscriber is in
    // place, so a failed init leaves them alone.
    global::set_text_map_propagator(crate::propagate::propagator_from_spec(
        config.propagators.as_deref(),
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
        exporter_failures,
    })
}

/// Whether this config will build a gRPC exporter, the one transport that
/// needs a Tokio runtime. False whenever nothing will be exported over gRPC:
/// another exporter, no endpoint for any signal, or every signal on HTTP.
#[cfg(feature = "otlp-grpc")]
fn needs_tokio_runtime(config: &TelemetryConfig) -> bool {
    use crate::config::{Exporter, Protocol, Signal};

    config.exporter == Exporter::Otlp
        && [Signal::Traces, Signal::Metrics, Signal::Logs]
            .into_iter()
            .any(|signal| {
                config
                    .otlp_target_for(signal)
                    .is_some_and(|target| target.protocol == Protocol::Grpc)
            })
}

/// An [`EnvFilter`] that gates events only: every span passes. On the log
/// layers, a filtered-out span must still be visible so the lines that do
/// pass keep their span context — the trace and span ids and the span fields.
/// Rejecting the span at the filter would erase it from those layers' view.
#[derive(Debug)]
struct EventsOnly(EnvFilter);

impl<S> tracing_subscriber::layer::Filter<S> for EventsOnly {
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        metadata.is_span() || tracing_subscriber::layer::Filter::<S>::enabled(&self.0, metadata, cx)
    }

    fn callsite_enabled(
        &self,
        metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        // The inner filter is always consulted, even though every span
        // passes: this is where it registers span callsites, without which
        // its span-scoped directives (`[request]=info`) could never match.
        let interest = tracing_subscriber::layer::Filter::<S>::callsite_enabled(&self.0, metadata);
        if metadata.is_span() {
            tracing::subscriber::Interest::always()
        } else {
            interest
        }
    }

    fn event_enabled(
        &self,
        event: &tracing::Event<'_>,
        cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        tracing_subscriber::layer::Filter::<S>::event_enabled(&self.0, event, cx)
    }

    // The span lifecycle is forwarded so the inner filter can track which
    // spans are current, which its span-scoped directives depend on.

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        tracing_subscriber::layer::Filter::<S>::on_new_span(&self.0, attrs, id, ctx);
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        tracing_subscriber::layer::Filter::<S>::on_record(&self.0, id, values, ctx);
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        tracing_subscriber::layer::Filter::<S>::on_enter(&self.0, id, ctx);
    }

    fn on_exit(&self, id: &tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        tracing_subscriber::layer::Filter::<S>::on_exit(&self.0, id, ctx);
    }

    fn on_close(&self, id: tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        tracing_subscriber::layer::Filter::<S>::on_close(&self.0, id, ctx);
    }

    // `max_level_hint` is deliberately not forwarded: the inner filter's hint
    // (say WARN) would let the registry skip INFO spans for this layer.
}

/// The per-layer filters. The log signal and the trace/metric signals are
/// gated separately, so quietening the stdout logs never drops telemetry.
#[derive(Debug)]
struct Filters {
    /// Stdout log lines: the configured log filter, events only.
    stdout: EventsOnly,
    /// Exported OTLP log records: the same directives, own instance.
    log_records: EventsOnly,
    /// Exported spans: the configured trace filter.
    spans: EnvFilter,
    /// `tracing`-field metrics: the trace filter directives, own instance.
    metrics: EnvFilter,
}

/// Builds every filter, so a directive that does not parse fails init before
/// any global state is touched.
fn filters(config: &TelemetryConfig) -> Result<Filters, InitError> {
    // The environment was already overlaid by `with_env`, so both filters
    // come straight from the config.
    let log_directives = config.log_filter.as_deref().unwrap_or("info");
    let trace_directives = config.trace_filter.as_deref().unwrap_or("info");

    let log_filter = |directives: &str| {
        EnvFilter::try_new(directives).map_err(|source| InitError::InvalidLogFilter {
            directives: directives.to_string(),
            source,
        })
    };
    let trace_filter = |directives: &str| {
        EnvFilter::try_new(directives).map_err(|source| InitError::InvalidTraceFilter {
            directives: directives.to_string(),
            source,
        })
    };

    Ok(Filters {
        stdout: EventsOnly(log_filter(log_directives)?),
        log_records: EventsOnly(log_filter(log_directives)?),
        spans: trace_filter(trace_directives)?,
        metrics: trace_filter(trace_directives)?,
    })
}

/// Stacks the stdout log layer and one layer per active provider on a
/// registry, each behind its signal's filter.
fn subscriber(
    config: &TelemetryConfig,
    providers: &Providers,
    filters: Filters,
) -> impl tracing::Subscriber + Send + Sync + 'static {
    subscriber_with(config, providers, filters, std::io::stdout)
}

/// [`subscriber`] with the stdout log layer writing to the given writer, so
/// tests can read the lines back.
fn subscriber_with<W>(
    config: &TelemetryConfig,
    providers: &Providers,
    filters: Filters,
    make_writer: W,
) -> impl tracing::Subscriber + Send + Sync + 'static
where
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    let otel_layer = providers.tracer.as_ref().map(|provider| {
        OpenTelemetryLayer::new(provider.tracer(config.service_name.clone()))
            .with_filter(filters.spans)
    });
    let metrics_layer = providers
        .meter
        .as_ref()
        .map(|provider| MetricsLayer::new(provider.clone()).with_filter(filters.metrics));

    // The log exporter's own network stack logs through tracing. Without the
    // target filter those events re-enter the bridge and feed back on
    // themselves.
    let log_bridge = providers.logger.as_ref().map(|provider| {
        OpenTelemetryTracingBridge::new(provider)
            .with_filter(
                Targets::new()
                    .with_default(LevelFilter::TRACE)
                    .with_target("opentelemetry", LevelFilter::OFF)
                    .with_target("tonic", LevelFilter::OFF)
                    .with_target("hyper", LevelFilter::OFF)
                    .with_target("h2", LevelFilter::OFF)
                    .with_target("tower", LevelFilter::OFF),
            )
            .with_filter(filters.log_records)
    });

    tracing_subscriber::registry()
        .with(fmt_layer_with(config.log_format, make_writer).with_filter(filters.stdout))
        .with(metrics_layer)
        .with(otel_layer)
        .with(log_bridge)
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

    fn in_memory_providers(spans: &InMemorySpanExporter, logs: &InMemoryLogExporter) -> Providers {
        Providers {
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
            failures: Vec::new(),
        }
    }

    #[test]
    fn the_log_filter_does_not_gate_spans() {
        let spans = InMemorySpanExporter::default();
        let logs = InMemoryLogExporter::default();
        let providers = in_memory_providers(&spans, &logs);
        // A service quietening its stdout logs must not lose its traces.
        let config = TelemetryConfig::new("svc").log_filter("warn");
        let subscriber = subscriber(
            &config,
            &providers,
            filters(&config).expect("valid filters"),
        );

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("work");
            let _enter = span.enter();
            tracing::info!("not a warning");
        });

        let finished = spans.get_finished_spans().expect("finished spans");
        assert_eq!(finished.len(), 1, "the log filter does not apply to spans");
        let records = logs.get_emitted_logs().expect("emitted logs");
        assert_eq!(records.len(), 0, "the log filter applies to log records");
    }

    #[test]
    fn the_trace_filter_gates_spans_independently() {
        let spans = InMemorySpanExporter::default();
        let logs = InMemoryLogExporter::default();
        let providers = in_memory_providers(&spans, &logs);
        let config = TelemetryConfig::new("svc")
            .log_filter("info")
            .trace_filter("warn");
        let subscriber = subscriber(
            &config,
            &providers,
            filters(&config).expect("valid filters"),
        );

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("work");
            let _enter = span.enter();
            tracing::info!("still logged");
        });

        let finished = spans.get_finished_spans().expect("finished spans");
        assert_eq!(finished.len(), 0, "the trace filter gates spans");
        let records = logs.get_emitted_logs().expect("emitted logs");
        assert_eq!(records.len(), 1, "the log filter is independent");
    }

    #[test]
    fn span_scoped_log_directives_still_match() {
        let spans = InMemorySpanExporter::default();
        let logs = InMemoryLogExporter::default();
        let providers = in_memory_providers(&spans, &logs);
        // INFO is suppressed in general but allowed inside a `request` span.
        let config = TelemetryConfig::new("svc")
            .log_filter("warn,[request]=info")
            .trace_filter("info");
        let subscriber = subscriber(
            &config,
            &providers,
            filters(&config).expect("valid filters"),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("outside any span");
            let span = tracing::info_span!("request");
            let _enter = span.enter();
            tracing::info!("inside the request span");
        });

        let records = logs.get_emitted_logs().expect("emitted logs");
        let bodies: Vec<String> = records
            .iter()
            .map(|log| format!("{:?}", log.record.body()))
            .collect();
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        assert!(bodies[0].contains("inside the request span"), "{bodies:?}");
    }

    #[test]
    fn the_trace_filter_gates_tracing_field_metrics() {
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        fn metrics_emitted(trace_filter: &str) -> bool {
            let exporter = InMemoryMetricExporter::default();
            let provider = SdkMeterProvider::builder()
                .with_reader(PeriodicReader::builder(exporter.clone()).build())
                .build();
            let providers = Providers {
                tracer: None,
                meter: Some(provider.clone()),
                logger: None,
                failures: Vec::new(),
            };
            let config = TelemetryConfig::new("svc")
                .log_filter("info")
                .trace_filter(trace_filter);
            let subscriber = subscriber(
                &config,
                &providers,
                filters(&config).expect("valid filters"),
            );

            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(monotonic_counter.requests = 1_u64, "one request");
            });

            provider.force_flush().expect("flush metrics");
            exporter
                .get_finished_metrics()
                .expect("finished metrics")
                .iter()
                .any(|resource_metrics| resource_metrics.scope_metrics().count() > 0)
        }

        assert!(metrics_emitted("info"), "an INFO metric event is exported");
        assert!(
            !metrics_emitted("warn"),
            "the trace filter gates metric events"
        );
    }

    #[test]
    fn an_invalid_trace_filter_is_reported() {
        let config = TelemetryConfig::new("svc").trace_filter("app=not-a-level");
        let err = filters(&config).expect_err("filter should not parse");
        assert!(matches!(err, InitError::InvalidTraceFilter { .. }), "{err}");
        assert!(err.to_string().contains("app=not-a-level"), "{err}");
    }

    #[test]
    fn log_records_carry_the_trace_id_of_the_span_they_were_emitted_in() {
        let spans = InMemorySpanExporter::default();
        let logs = InMemoryLogExporter::default();
        let providers = in_memory_providers(&spans, &logs);
        let config = TelemetryConfig::new("svc").log_filter("info");
        let subscriber = subscriber(
            &config,
            &providers,
            filters(&config).expect("valid filters"),
        );

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
        fn a_quiet_log_filter_keeps_span_context_on_the_lines_it_lets_through() {
            let buffer = Buffer::default();
            let spans = InMemorySpanExporter::default();
            let logs = InMemoryLogExporter::default();
            let providers = in_memory_providers(&spans, &logs);
            // The reviewer's repro: quiet logs, normal traces, JSON output.
            let config = TelemetryConfig::new("svc")
                .log_filter("warn")
                .trace_filter("info")
                .log_format(LogFormat::Json);
            let subscriber = subscriber_with(
                &config,
                &providers,
                filters(&config).expect("valid filters"),
                buffer.clone(),
            );

            tracing::subscriber::with_default(subscriber, || {
                let span = tracing::info_span!("request", method = "GET");
                let _enter = span.enter();
                tracing::warn!("something noteworthy");
            });

            let finished = spans.get_finished_spans().expect("finished spans");
            assert_eq!(finished.len(), 1, "the INFO span is exported");
            let trace_id = finished[0].span_context.trace_id().to_string();

            // The WARN line that passed the log filter keeps the span context
            // the filter would have hidden: the ids and the span fields.
            let output = buffer.contents();
            let line = output
                .lines()
                .find(|line| line.contains("something noteworthy"))
                .expect("the WARN event was printed");
            let json: serde_json::Value =
                serde_json::from_str(line).expect("one JSON object per line");
            assert_eq!(json["trace_id"].as_str(), Some(trace_id.as_str()), "{json}");
            assert_eq!(json["span_id"].as_str().map(str::len), Some(16), "{json}");
            assert_eq!(json["span"]["name"].as_str(), Some("request"), "{json}");
            assert_eq!(json["span"]["method"].as_str(), Some("GET"), "{json}");

            // The exported log record keeps its trace correlation too.
            let records = logs.get_emitted_logs().expect("emitted logs");
            assert_eq!(records.len(), 1, "only the WARN event is exported");
            let context = records[0]
                .record
                .trace_context()
                .expect("log record has a trace context");
            assert_eq!(context.trace_id.to_string(), trace_id);
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
            let subscriber = tracing_subscriber::registry()
                .with(fmt_layer_with(LogFormat::Json, buffer.clone()));

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
        let err = filters(&config).expect_err("filter should not parse");
        assert!(matches!(err, InitError::InvalidLogFilter { .. }), "{err}");
        assert!(err.to_string().contains("app=not-a-level"), "{err}");
    }
}
