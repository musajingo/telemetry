//! Construction of the trace, metric and log providers.

// With no exporter feature enabled the provider builders have no caller.
#![cfg_attr(
    not(any(feature = "otlp-grpc", feature = "otlp-http", feature = "stdout")),
    allow(dead_code, unused_imports)
)]

#[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
use opentelemetry_otlp::{ExporterBuildError, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::{PeriodicReader, SdkMeterProvider, exporter::PushMetricExporter},
    trace::{Sampler, SdkTracerProvider, TracerProviderBuilder},
};

use crate::config::{Exporter, TelemetryConfig};
#[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
use crate::config::{Protocol, Signal, SignalTarget};

/// The providers behind one telemetry setup. A `None` means that signal is not
/// exported.
#[derive(Default)]
pub(crate) struct Providers {
    pub(crate) tracer: Option<SdkTracerProvider>,
    pub(crate) meter: Option<SdkMeterProvider>,
    pub(crate) logger: Option<SdkLoggerProvider>,
}

impl Providers {
    /// Builds the providers the config asks for. Touches no global state.
    pub(crate) fn build(config: &TelemetryConfig) -> Self {
        match config.exporter {
            Exporter::Otlp => Self::otlp(config),
            #[cfg(feature = "stdout")]
            Exporter::Stdout => Self::stdout(config),
            Exporter::None => Self::default(),
        }
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    fn otlp(config: &TelemetryConfig) -> Self {
        let resource = config.resource();

        let tracer = config
            .otlp_target_for(Signal::Traces)
            .and_then(|target| {
                span_exporter(&target)
                    .inspect_err(|err| {
                        eprintln!("telemetry: OTLP span exporter disabled ({err:?})");
                    })
                    .ok()
            })
            .map(|exporter| {
                tracer_provider_builder(config, resource.clone())
                    .with_batch_exporter(exporter)
                    .build()
            });
        let meter = config
            .otlp_target_for(Signal::Metrics)
            .and_then(|target| {
                metric_exporter(&target)
                    .inspect_err(|err| {
                        eprintln!("telemetry: OTLP metric exporter disabled ({err:?})");
                    })
                    .ok()
            })
            .map(|exporter| meter_provider(config, exporter, resource.clone()));
        // Log records flow from `tracing` events through the
        // `OpenTelemetryTracingBridge` into this provider.
        let logger = config
            .otlp_target_for(Signal::Logs)
            .and_then(|target| {
                log_exporter(&target)
                    .inspect_err(|err| {
                        eprintln!("telemetry: OTLP log exporter disabled ({err:?})");
                    })
                    .ok()
            })
            .map(|exporter| {
                SdkLoggerProvider::builder()
                    .with_resource(resource)
                    .with_batch_exporter(exporter)
                    .build()
            });

        Self {
            tracer,
            meter,
            logger,
        }
    }

    #[cfg(not(any(feature = "otlp-grpc", feature = "otlp-http")))]
    fn otlp(config: &TelemetryConfig) -> Self {
        if config.otlp_endpoint.is_some() {
            eprintln!(
                "telemetry: an OTLP endpoint is configured but neither the `otlp-grpc` nor the `otlp-http` feature is enabled, nothing will be exported"
            );
        }
        Self::default()
    }

    /// Spans are written as they end and metrics on the usual interval. Log
    /// records are left to the stdout log layer, which prints them already.
    #[cfg(feature = "stdout")]
    fn stdout(config: &TelemetryConfig) -> Self {
        let resource = config.resource();
        let tracer = tracer_provider_builder(config, resource.clone())
            .with_simple_exporter(opentelemetry_stdout::SpanExporter::default())
            .build();
        let meter = meter_provider(
            config,
            opentelemetry_stdout::MetricExporter::default(),
            resource,
        );
        Self {
            tracer: Some(tracer),
            meter: Some(meter),
            logger: None,
        }
    }
}

fn tracer_provider_builder(config: &TelemetryConfig, resource: Resource) -> TracerProviderBuilder {
    let builder = SdkTracerProvider::builder().with_resource(resource);
    match config.sample_ratio {
        Some(ratio) => builder.with_sampler(Sampler::ParentBased(Box::new(
            Sampler::TraceIdRatioBased(ratio),
        ))),
        // The SDK default honours OTEL_TRACES_SAMPLER and samples every trace
        // otherwise.
        None => builder,
    }
}

fn meter_provider(
    config: &TelemetryConfig,
    exporter: impl PushMetricExporter,
    resource: Resource,
) -> SdkMeterProvider {
    let reader = PeriodicReader::builder(exporter);
    let reader = match config.metrics_interval {
        Some(interval) => reader.with_interval(interval),
        // The SDK default honours OTEL_METRIC_EXPORT_INTERVAL.
        None => reader,
    };

    SdkMeterProvider::builder()
        .with_resource(resource)
        .with_reader(reader.build())
        .build()
}

/// Over HTTP each signal has its own path under the base URL. The exporter
/// only appends it to an endpoint it read from the environment, not to one
/// handed to it in code.
#[cfg(feature = "otlp-http")]
fn signal_url(endpoint: &str, path: &str) -> String {
    format!("{}/{path}", endpoint.trim_end_matches('/'))
}

/// The HTTP URL for one signal: a per-signal endpoint is used exactly as
/// given, the base endpoint gets the signal's path appended, as the OTLP
/// specification says.
#[cfg(feature = "otlp-http")]
fn http_url(target: &SignalTarget, path: &str) -> String {
    if target.exact {
        target.endpoint.clone()
    } else {
        signal_url(&target.endpoint, path)
    }
}

#[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
fn span_exporter(
    target: &SignalTarget,
) -> Result<opentelemetry_otlp::SpanExporter, ExporterBuildError> {
    let builder = opentelemetry_otlp::SpanExporter::builder();
    match target.protocol {
        #[cfg(feature = "otlp-grpc")]
        Protocol::Grpc => builder.with_tonic().with_endpoint(&target.endpoint).build(),
        #[cfg(feature = "otlp-http")]
        Protocol::HttpProtobuf => builder
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .with_endpoint(http_url(target, "v1/traces"))
            .build(),
    }
}

#[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
fn metric_exporter(
    target: &SignalTarget,
) -> Result<opentelemetry_otlp::MetricExporter, ExporterBuildError> {
    let builder = opentelemetry_otlp::MetricExporter::builder();
    match target.protocol {
        #[cfg(feature = "otlp-grpc")]
        Protocol::Grpc => builder.with_tonic().with_endpoint(&target.endpoint).build(),
        #[cfg(feature = "otlp-http")]
        Protocol::HttpProtobuf => builder
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .with_endpoint(http_url(target, "v1/metrics"))
            .build(),
    }
}

#[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
fn log_exporter(
    target: &SignalTarget,
) -> Result<opentelemetry_otlp::LogExporter, ExporterBuildError> {
    let builder = opentelemetry_otlp::LogExporter::builder();
    match target.protocol {
        #[cfg(feature = "otlp-grpc")]
        Protocol::Grpc => builder.with_tonic().with_endpoint(&target.endpoint).build(),
        #[cfg(feature = "otlp-http")]
        Protocol::HttpProtobuf => builder
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .with_endpoint(http_url(target, "v1/logs"))
            .build(),
    }
}

#[cfg(all(test, feature = "otlp-http"))]
mod tests {
    use super::*;

    #[test]
    fn signal_paths_are_appended_to_the_base_url() {
        assert_eq!(
            signal_url("http://collector:4318", "v1/traces"),
            "http://collector:4318/v1/traces"
        );
        assert_eq!(
            signal_url("https://gateway.example.com/otlp/", "v1/logs"),
            "https://gateway.example.com/otlp/v1/logs"
        );
    }

    #[test]
    fn a_per_signal_endpoint_is_used_verbatim() {
        use crate::config::SignalTarget;

        let exact = SignalTarget {
            protocol: Protocol::HttpProtobuf,
            endpoint: "http://traces.internal:4318/custom/traces".to_string(),
            exact: true,
        };
        assert_eq!(
            http_url(&exact, "v1/traces"),
            "http://traces.internal:4318/custom/traces"
        );

        let base = SignalTarget {
            protocol: Protocol::HttpProtobuf,
            endpoint: "http://collector:4318".to_string(),
            exact: false,
        };
        assert_eq!(
            http_url(&base, "v1/traces"),
            "http://collector:4318/v1/traces"
        );
    }
}
