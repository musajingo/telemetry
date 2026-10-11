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
    let layer = tracing_subscriber::fmt::layer();
    match format {
        LogFormat::Text => layer.boxed(),
        LogFormat::Pretty => layer.pretty().boxed(),
        #[cfg(feature = "json")]
        LogFormat::Json => layer.json().boxed(),
    }
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

    #[test]
    fn an_invalid_log_filter_is_reported() {
        let config = TelemetryConfig::new("svc").log_filter("app=not-a-level");
        let err = env_filter(&config).expect_err("filter should not parse");
        assert!(matches!(err, InitError::InvalidLogFilter { .. }), "{err}");
        assert!(err.to_string().contains("app=not-a-level"), "{err}");
    }
}
