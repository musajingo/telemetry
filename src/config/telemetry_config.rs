//! The config builder: defaults, builder-style setters and the environment
//! overlay.

use std::time::Duration;

use opentelemetry::{Key, KeyValue, Value};
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::{
    SCHEMA_URL,
    attribute::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_NAME, SERVICE_VERSION},
};

use super::exporter::Exporter;
use super::log_format::LogFormat;
use super::protocol::Protocol;
use super::signal::{Signal, SignalOverride, SignalTarget};
use crate::error::InitError;
use crate::guard::TelemetryGuard;

/// Everything [`try_init`](Self::try_init) needs to set telemetry up.
///
/// Start from [`new`](Self::new) for pure defaults or [`from_env`](Self::from_env)
/// to also read the environment, then adjust with the setters. Precedence is
/// call order, per setting: [`with_env`](Self::with_env) overrides setters
/// called before it, setters called after it override the environment.
///
/// ```
/// use telemetry::{LogFormat, TelemetryConfig};
///
/// let config = TelemetryConfig::new(env!("CARGO_PKG_NAME"))
///     .with_env()
///     .log_format(LogFormat::Pretty);
/// # let _ = config;
/// ```
#[derive(Clone, Debug)]
#[must_use = "a config does nothing until `init` or `try_init` is called"]
pub struct TelemetryConfig {
    pub(crate) service_name: String,
    pub(crate) service_version: Option<String>,
    pub(crate) environment: Option<String>,
    pub(crate) resource_attributes: Vec<KeyValue>,
    pub(crate) exporter: Exporter,
    pub(crate) otlp_endpoint: Option<String>,
    pub(crate) otlp_protocol: Option<Protocol>,
    pub(crate) otlp_traces: SignalOverride,
    pub(crate) otlp_metrics: SignalOverride,
    pub(crate) otlp_logs: SignalOverride,
    pub(crate) sample_ratio: Option<f64>,
    pub(crate) metrics_export_interval: Option<Duration>,
    pub(crate) log_filter: Option<String>,
    pub(crate) trace_filter: Option<String>,
    pub(crate) propagators: Option<String>,
    pub(crate) log_format: LogFormat,
    pub(crate) trust_incoming_trace: bool,
    pub(crate) capture_panics: bool,
    pub(crate) strict: bool,
}

impl TelemetryConfig {
    /// A config with the given service name and default settings. Reads no
    /// environment variables itself (the SDK still reads its standard ones
    /// during init; see [`with_env`](Self::with_env)).
    ///
    /// The defaults: OTLP export (inactive until an endpoint is set) over
    /// gRPC when the `otlp-grpc` feature is on, the SDK's sampler and metric
    /// interval, text logs, both filters at `info`, both W3C propagators,
    /// inbound trace context not trusted, panic hook installed.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: None,
            environment: None,
            resource_attributes: Vec::new(),
            exporter: Exporter::default(),
            otlp_endpoint: None,
            otlp_protocol: None,
            otlp_traces: SignalOverride::default(),
            otlp_metrics: SignalOverride::default(),
            otlp_logs: SignalOverride::default(),
            sample_ratio: None,
            metrics_export_interval: None,
            log_filter: None,
            trace_filter: None,
            propagators: None,
            log_format: LogFormat::default(),
            trust_incoming_trace: false,
            capture_panics: true,
            strict: false,
        }
    }

    /// Shorthand for `TelemetryConfig::new(service_name).with_env()`: the
    /// given name is used unless `OTEL_SERVICE_NAME` is set.
    pub fn from_env(service_name: impl Into<String>) -> Self {
        Self::new(service_name).with_env()
    }

    /// Overlays the crate-managed environment settings on top of the
    /// current values. Unset, blank and unparsable variables leave the
    /// current value alone — except the filter and propagator variables,
    /// which are stored as given and validated during init.
    ///
    /// | Variable | Setting |
    /// | --- | --- |
    /// | `OTEL_SERVICE_NAME` | [`service_name`](Self::service_name) |
    /// | `OTEL_EXPORTER_OTLP_ENDPOINT` | [`otlp_endpoint`](Self::otlp_endpoint) |
    /// | `OTEL_EXPORTER_OTLP_PROTOCOL` | [`otlp_protocol`](Self::otlp_protocol): `grpc` or `http/protobuf` |
    /// | `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT` | [`otlp_traces_endpoint`](Self::otlp_traces_endpoint) and siblings |
    /// | `OTEL_EXPORTER_OTLP_<SIGNAL>_PROTOCOL` | [`otlp_traces_protocol`](Self::otlp_traces_protocol) and siblings |
    /// | `OTEL_SDK_DISABLED` | `true` selects [`Exporter::None`] |
    /// | `RUST_LOG` | [`log_filter`](Self::log_filter) |
    /// | `TELEMETRY_TRACE_FILTER` | [`trace_filter`](Self::trace_filter) |
    /// | `OTEL_PROPAGATORS` | [`propagators`](Self::propagators) |
    /// | `TELEMETRY_LOG_FORMAT` | [`log_format`](Self::log_format): `text`, `pretty` or `json` |
    /// | `TELEMETRY_TRUST_INCOMING_TRACE` | [`trust_incoming_trace`](Self::trust_incoming_trace) |
    /// | `TELEMETRY_STRICT` | [`strict`](Self::strict) |
    ///
    /// Separately, the SDK reads its own standard variables during init
    /// (`OTEL_RESOURCE_ATTRIBUTES`, the sampler pair, the metric interval and
    /// exporter tuning), even without this overlay; the matching code setters
    /// override those regardless of call order. The crate documentation has
    /// the full table.
    pub fn with_env(self) -> Self {
        self.with_env_from(|key| std::env::var(key).ok())
    }

    /// [`with_env`](Self::with_env) against an arbitrary variable source.
    pub(crate) fn with_env_from(mut self, lookup: impl Fn(&str) -> Option<String>) -> Self {
        let var = |key: &str| {
            lookup(key)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };

        if let Some(name) = var("OTEL_SERVICE_NAME") {
            self.service_name = name;
        }
        if let Some(endpoint) = var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            self.otlp_endpoint = Some(endpoint);
        }
        if let Some(value) = var("OTEL_EXPORTER_OTLP_PROTOCOL") {
            match Protocol::parse(&value) {
                Some(protocol) => self.otlp_protocol = Some(protocol),
                None => eprintln!(
                    "telemetry: ignoring OTEL_EXPORTER_OTLP_PROTOCOL={value:?} (unsupported protocol, or its feature is disabled)"
                ),
            }
        }
        for (signal, overrides) in [
            ("TRACES", &mut self.otlp_traces),
            ("METRICS", &mut self.otlp_metrics),
            ("LOGS", &mut self.otlp_logs),
        ] {
            if let Some(endpoint) = var(&format!("OTEL_EXPORTER_OTLP_{signal}_ENDPOINT")) {
                overrides.endpoint = Some(endpoint);
            }
            if let Some(value) = var(&format!("OTEL_EXPORTER_OTLP_{signal}_PROTOCOL")) {
                match Protocol::parse(&value) {
                    Some(protocol) => overrides.protocol = Some(protocol),
                    None => eprintln!(
                        "telemetry: ignoring OTEL_EXPORTER_OTLP_{signal}_PROTOCOL={value:?} (unsupported protocol, or its feature is disabled)"
                    ),
                }
            }
        }
        if var("OTEL_SDK_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
            self.exporter = Exporter::None;
        }

        if let Some(filter) = var("RUST_LOG") {
            self.log_filter = Some(filter);
        }
        if let Some(filter) = var("TELEMETRY_TRACE_FILTER") {
            self.trace_filter = Some(filter);
        }
        if let Some(propagators) = var("OTEL_PROPAGATORS") {
            self.propagators = Some(propagators);
        }
        if let Some(value) = var("TELEMETRY_LOG_FORMAT") {
            match LogFormat::parse(&value) {
                Some(format) => self.log_format = format,
                None => eprintln!(
                    "telemetry: ignoring TELEMETRY_LOG_FORMAT={value:?} (unknown format, or its feature is disabled)"
                ),
            }
        }
        if let Some(trust) = var("TELEMETRY_TRUST_INCOMING_TRACE").and_then(|v| parse_bool(&v)) {
            self.trust_incoming_trace = trust;
        }
        if let Some(strict) = var("TELEMETRY_STRICT").and_then(|v| parse_bool(&v)) {
            self.strict = strict;
        }

        self
    }

    /// Sets `service.name`. It is also used as the tracer (instrumentation
    /// scope) name.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Sets `service.version`; pass `env!("CARGO_PKG_VERSION")` from the
    /// application. Wins over `OTEL_RESOURCE_ATTRIBUTES`. Left unset, that
    /// variable applies, or the attribute is omitted.
    pub fn service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Sets `deployment.environment.name`, e.g. `production` or `staging`.
    /// Wins over `OTEL_RESOURCE_ATTRIBUTES`. Left unset, that variable
    /// applies, or the attribute is omitted.
    pub fn environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = Some(environment.into());
        self
    }

    /// Adds one more resource attribute, e.g. `cloud.region`. For a key set
    /// several ways, the last call to this setter wins, over
    /// `OTEL_RESOURCE_ATTRIBUTES` and over the dedicated setters.
    pub fn resource_attribute(mut self, key: impl Into<Key>, value: impl Into<Value>) -> Self {
        self.resource_attributes.push(KeyValue::new(key, value));
        self
    }

    /// Chooses where telemetry is exported. Defaults to [`Exporter::Otlp`].
    /// Call after [`with_env`](Self::with_env) to override `OTEL_SDK_DISABLED`.
    pub fn exporter(mut self, exporter: Exporter) -> Self {
        self.exporter = exporter;
        self
    }

    /// Sets the default collector endpoint, e.g. `http://localhost:4317`,
    /// used by every signal without its own endpoint. Per-signal endpoints
    /// set earlier by code or environment are kept; to replace those too,
    /// use [`otlp_endpoint_for_all`](Self::otlp_endpoint_for_all).
    ///
    /// Over HTTP the standard `/v1/traces`, `/v1/metrics` and `/v1/logs`
    /// paths are appended to it. An `https://` endpoint needs the
    /// `otlp-grpc-tls` or `otlp-http-tls` feature.
    pub fn otlp_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(endpoint.into());
        self
    }

    /// [`otlp_endpoint`](Self::otlp_endpoint), but also discards every
    /// per-signal endpoint set earlier, so all three signals go here. After
    /// [`with_env`](Self::with_env), this is the one that guarantees no
    /// environment-provided endpoint still redirects a signal.
    pub fn otlp_endpoint_for_all(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_traces.endpoint = None;
        self.otlp_metrics.endpoint = None;
        self.otlp_logs.endpoint = None;
        self.otlp_endpoint(endpoint)
    }

    /// Sends spans to their own endpoint, used exactly as given (no
    /// `/v1/traces` appended). Overrides the base endpoint for this signal.
    pub fn otlp_traces_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_traces.endpoint = Some(endpoint.into());
        self
    }

    /// Sends metrics to their own endpoint, used exactly as given (no
    /// `/v1/metrics` appended). Overrides the base endpoint for this signal.
    pub fn otlp_metrics_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_metrics.endpoint = Some(endpoint.into());
        self
    }

    /// Sends log records to their own endpoint, used exactly as given (no
    /// `/v1/logs` appended). Overrides the base endpoint for this signal.
    pub fn otlp_logs_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_logs.endpoint = Some(endpoint.into());
        self
    }

    /// Sets the default OTLP transport, used by every signal without its own.
    /// Per-signal protocols set earlier are kept; to replace those too, use
    /// [`otlp_protocol_for_all`](Self::otlp_protocol_for_all). Defaults to
    /// gRPC when the `otlp-grpc` feature is enabled and to HTTP otherwise.
    pub fn otlp_protocol(mut self, protocol: Protocol) -> Self {
        self.otlp_protocol = Some(protocol);
        self
    }

    /// [`otlp_protocol`](Self::otlp_protocol), but also discards every
    /// per-signal protocol set earlier, so all three signals use this one.
    pub fn otlp_protocol_for_all(mut self, protocol: Protocol) -> Self {
        self.otlp_traces.protocol = None;
        self.otlp_metrics.protocol = None;
        self.otlp_logs.protocol = None;
        self.otlp_protocol(protocol)
    }

    /// Chooses the transport for spans only.
    pub fn otlp_traces_protocol(mut self, protocol: Protocol) -> Self {
        self.otlp_traces.protocol = Some(protocol);
        self
    }

    /// Chooses the transport for metrics only.
    pub fn otlp_metrics_protocol(mut self, protocol: Protocol) -> Self {
        self.otlp_metrics.protocol = Some(protocol);
        self
    }

    /// Chooses the transport for log records only.
    pub fn otlp_logs_protocol(mut self, protocol: Protocol) -> Self {
        self.otlp_logs.protocol = Some(protocol);
        self
    }

    /// Samples this fraction of new traces, clamped to `[0.0, 1.0]`. The
    /// sampler is parent-based, so a sampled upstream trace is always
    /// continued.
    ///
    /// Wins over `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG`. Left
    /// unset, those apply, or every new root trace is sampled.
    pub fn sample_ratio(mut self, ratio: f64) -> Self {
        if !ratio.is_nan() {
            self.sample_ratio = Some(ratio.clamp(0.0, 1.0));
        }
        self
    }

    /// Sets how often metrics are exported. A nonzero interval wins over
    /// `OTEL_METRIC_EXPORT_INTERVAL` (milliseconds); zero or unset leaves the
    /// SDK default, 60 seconds.
    pub fn metrics_export_interval(mut self, interval: Duration) -> Self {
        self.metrics_export_interval = Some(interval);
        self
    }

    /// Sets the log filter, in `RUST_LOG` syntax (`info,sqlx=warn`). It
    /// applies to the log signal only: the stdout log lines and the exported
    /// OTLP log records. Spans and metrics are gated by
    /// [`trace_filter`](Self::trace_filter), so quietening the logs never
    /// drops traces.
    ///
    /// Left unset, `info`. [`with_env`](Self::with_env) overlays `RUST_LOG`
    /// onto this.
    pub fn log_filter(mut self, directives: impl Into<String>) -> Self {
        self.log_filter = Some(directives.into());
        self
    }

    /// Sets the filter for exported spans and `tracing`-field metrics, in
    /// `RUST_LOG` syntax. Independent of [`log_filter`](Self::log_filter):
    /// raising one never silences the other.
    ///
    /// Left unset, `info`. [`with_env`](Self::with_env) overlays
    /// `TELEMETRY_TRACE_FILTER` onto this.
    pub fn trace_filter(mut self, directives: impl Into<String>) -> Self {
        self.trace_filter = Some(directives.into());
        self
    }

    /// Chooses the propagators that carry context across services:
    /// `tracecontext`, `baggage` or `none`, comma-separated. Unsupported
    /// entries are reported during init and skipped.
    ///
    /// Left unset, both W3C propagators are installed.
    /// [`with_env`](Self::with_env) overlays `OTEL_PROPAGATORS` onto this.
    pub fn propagators(mut self, propagators: impl Into<String>) -> Self {
        self.propagators = Some(propagators.into());
        self
    }

    /// Sets the stdout log format. Defaults to [`LogFormat::Text`].
    pub fn log_format(mut self, format: LogFormat) -> Self {
        self.log_format = format;
        self
    }

    /// Whether `OtelMakeSpan` continues the trace named by an inbound
    /// `traceparent` header. Defaults to `false`.
    ///
    /// Whoever controls the parent context controls the sampling decision
    /// (and so the cost) and the trace identity. Enable this only where every
    /// caller is trusted, or where a proxy in front of the service strips
    /// trace headers sent by clients.
    pub fn trust_incoming_trace(mut self, trust: bool) -> Self {
        self.trust_incoming_trace = trust;
        self
    }

    /// Whether to report panics through `tracing`. Defaults to `true`.
    pub fn capture_panics(mut self, capture: bool) -> Self {
        self.capture_panics = capture;
        self
    }

    /// Whether init fails when OTLP export will not happen: no endpoint for
    /// any signal, or an exporter that cannot be built. Defaults to `false`,
    /// which reports the problem and boots without that signal.
    /// `Exporter::Stdout` and [`Exporter::None`] are deliberate choices and
    /// stay allowed.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Installs the global subscriber and returns the guard that owns the
    /// telemetry providers.
    ///
    /// # Panics
    ///
    /// Panics where [`try_init`](Self::try_init) would return an error.
    pub fn init(self) -> TelemetryGuard {
        match self.try_init() {
            Ok(guard) => guard,
            Err(err) => panic!("failed to initialize telemetry: {err}"),
        }
    }

    /// Installs the global subscriber and returns the guard that owns the
    /// telemetry providers.
    ///
    /// An exporter that cannot be built is reported on stderr and that
    /// signal is disabled, rather than failing the whole service over its
    /// telemetry.
    ///
    /// # Errors
    ///
    /// Fails, leaving every global untouched, when a global `tracing`
    /// subscriber is already installed, the configured log or trace filter
    /// does not parse, `Protocol::Grpc` export is configured outside a
    /// Tokio runtime (which the gRPC client needs), or [`strict`](Self::strict)
    /// mode found that OTLP export will not happen.
    pub fn try_init(self) -> Result<TelemetryGuard, InitError> {
        crate::init::install(self)
    }

    /// Where one signal's exporter sends to, honouring that signal's
    /// `OTEL_EXPORTER_OTLP_<SIGNAL>_*` overrides before the base settings.
    /// `None` when no endpoint applies to the signal, which leaves it off.
    #[cfg_attr(
        not(any(feature = "otlp-grpc", feature = "otlp-http")),
        allow(dead_code)
    )]
    pub(crate) fn otlp_target_for(&self, signal: Signal) -> Option<SignalTarget> {
        let overrides = match signal {
            Signal::Traces => &self.otlp_traces,
            Signal::Metrics => &self.otlp_metrics,
            Signal::Logs => &self.otlp_logs,
        };
        let protocol = overrides
            .protocol
            .or(self.otlp_protocol)
            .or_else(Protocol::preferred)?;
        let (endpoint, exact) = match &overrides.endpoint {
            Some(endpoint) => (endpoint.clone(), true),
            None => (self.otlp_endpoint.clone()?, false),
        };
        Some(SignalTarget {
            protocol,
            endpoint,
            exact,
        })
    }

    /// The resource describing this service.
    ///
    /// Starts from the SDK's detectors (which read `OTEL_RESOURCE_ATTRIBUTES`)
    /// and layers the configured attributes on top.
    #[cfg_attr(
        not(any(feature = "otlp-grpc", feature = "otlp-http", feature = "stdout")),
        allow(dead_code)
    )]
    pub(crate) fn resource(&self) -> Resource {
        let mut attributes = vec![KeyValue::new(SERVICE_NAME, self.service_name.clone())];
        if let Some(version) = &self.service_version {
            attributes.push(KeyValue::new(SERVICE_VERSION, version.clone()));
        }
        if let Some(environment) = &self.environment {
            attributes.push(KeyValue::new(
                DEPLOYMENT_ENVIRONMENT_NAME,
                environment.clone(),
            ));
        }
        attributes.extend(self.resource_attributes.iter().cloned());

        // `with_schema_url` lets the detectors win over the attributes it is
        // given, which would turn `service.name` into `unknown_service`. It
        // only sets the schema here, and the attributes go in afterwards.
        Resource::builder()
            .with_schema_url([], SCHEMA_URL)
            .with_attributes(attributes)
            .build()
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn overlay(config: TelemetryConfig, vars: &[(&str, &str)]) -> TelemetryConfig {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        config.with_env_from(|key| vars.get(key).cloned())
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    fn endpoint(config: &TelemetryConfig) -> Option<String> {
        config
            .otlp_target_for(Signal::Traces)
            .map(|target| target.endpoint)
    }

    fn attribute(resource: &Resource, key: &'static str) -> Option<String> {
        resource
            .get(&Key::from_static_str(key))
            .map(|v| v.to_string())
    }

    #[test]
    fn an_empty_environment_changes_nothing() {
        let config = overlay(TelemetryConfig::new("svc").service_version("1.2.3"), &[]);

        assert_eq!(config.service_name, "svc");
        assert_eq!(config.service_version.as_deref(), Some("1.2.3"));
        assert_eq!(config.environment, None);
        assert_eq!(config.exporter, Exporter::Otlp);
        assert_eq!(config.otlp_endpoint, None);
        assert_eq!(config.sample_ratio, None);
        assert_eq!(config.log_format, LogFormat::Text);
        assert!(!config.trust_incoming_trace);
    }

    #[test]
    fn the_environment_overrides_earlier_setters_and_yields_to_later_ones() {
        let config = overlay(
            TelemetryConfig::new("fallback"),
            &[("OTEL_SERVICE_NAME", "from-env")],
        );

        assert_eq!(config.service_name, "from-env");
        let config = config.service_name("from-code");
        assert_eq!(config.service_name, "from-code");
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn the_standard_endpoint_is_trimmed() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_EXPORTER_OTLP_ENDPOINT", " http://collector:4317 ")],
        );
        assert_eq!(endpoint(&config).as_deref(), Some("http://collector:4317"));
    }

    #[test]
    fn nonstandard_otel_aliases_are_not_read() {
        let config = overlay(
            TelemetryConfig::new("svc")
                .service_version("code-version")
                .environment("production")
                .sample_ratio(0.25),
            &[
                ("OTEL_HOST", "collector"),
                ("OTEL_PORT", "14317"),
                ("OTEL_SERVICE_VERSION", "env-version"),
                ("ENVIRONMENT", "staging"),
                ("OTEL_TRACES_SAMPLE_RATIO", "0"),
            ],
        );
        assert_eq!(config.otlp_endpoint, None);
        assert_eq!(config.service_version.as_deref(), Some("code-version"));
        assert_eq!(config.environment.as_deref(), Some("production"));
        assert_eq!(config.sample_ratio, Some(0.25));
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn a_per_signal_endpoint_overrides_the_base_for_that_signal_only() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"),
                (
                    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                    "http://traces.internal:4317",
                ),
            ],
        );

        let traces = config.otlp_target_for(Signal::Traces).expect("traces");
        assert_eq!(traces.endpoint, "http://traces.internal:4317");
        assert!(traces.exact, "a per-signal endpoint is used as given");

        let metrics = config.otlp_target_for(Signal::Metrics).expect("metrics");
        assert_eq!(metrics.endpoint, "http://collector:4317");
        assert!(!metrics.exact, "the base endpoint gets the signal path");
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn a_per_signal_endpoint_alone_enables_only_that_signal() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[(
                "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                "http://logs.internal:4317",
            )],
        );

        assert!(config.otlp_target_for(Signal::Logs).is_some());
        assert!(config.otlp_target_for(Signal::Traces).is_none());
        assert!(config.otlp_target_for(Signal::Metrics).is_none());
    }

    #[cfg(all(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn a_per_signal_protocol_overrides_the_base_protocol() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"),
                ("OTEL_EXPORTER_OTLP_METRICS_PROTOCOL", "http/protobuf"),
            ],
        );

        let traces = config.otlp_target_for(Signal::Traces).expect("traces");
        assert_eq!(traces.protocol, Protocol::Grpc);
        assert_eq!(traces.endpoint, "http://collector:4317");

        // A protocol override preserves the explicitly configured endpoint.
        let metrics = config.otlp_target_for(Signal::Metrics).expect("metrics");
        assert_eq!(metrics.protocol, Protocol::HttpProtobuf);
        assert_eq!(metrics.endpoint, "http://collector:4317");
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn per_signal_setters_refine_the_base() {
        let config = TelemetryConfig::new("svc")
            .otlp_endpoint("http://collector:4317")
            .otlp_traces_endpoint("http://traces.internal:4317/v1/traces");

        let traces = config.otlp_target_for(Signal::Traces).expect("traces");
        assert_eq!(traces.endpoint, "http://traces.internal:4317/v1/traces");
        assert!(traces.exact, "a per-signal endpoint is used as given");

        let metrics = config.otlp_target_for(Signal::Metrics).expect("metrics");
        assert_eq!(metrics.endpoint, "http://collector:4317");
        assert!(!metrics.exact);
    }

    #[cfg(all(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn per_signal_protocol_setters_refine_the_base() {
        let config = TelemetryConfig::new("svc")
            .otlp_endpoint("http://collector:4317")
            .otlp_logs_protocol(Protocol::HttpProtobuf);

        assert_eq!(
            config.otlp_target_for(Signal::Logs).expect("logs").protocol,
            Protocol::HttpProtobuf
        );
        assert_eq!(
            config
                .otlp_target_for(Signal::Traces)
                .expect("traces")
                .protocol,
            Protocol::Grpc
        );
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn the_base_setters_preserve_per_signal_overrides() {
        // An override set in code survives a later change of the default...
        let config = TelemetryConfig::new("svc")
            .otlp_traces_endpoint("http://traces.internal:4317")
            .otlp_endpoint("http://collector:4317");

        let traces = config.otlp_target_for(Signal::Traces).expect("traces");
        assert_eq!(traces.endpoint, "http://traces.internal:4317");
        assert!(traces.exact);
        let metrics = config.otlp_target_for(Signal::Metrics).expect("metrics");
        assert_eq!(metrics.endpoint, "http://collector:4317");

        // ...and so does one from the environment.
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "http://logs.env:4317")],
        )
        .otlp_endpoint("http://collector:4317");
        let logs = config.otlp_target_for(Signal::Logs).expect("logs");
        assert_eq!(logs.endpoint, "http://logs.env:4317");
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn for_all_replaces_overrides_from_code_and_environment() {
        let config = overlay(
            TelemetryConfig::new("svc").otlp_traces_endpoint("http://traces.code:4317"),
            &[("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "http://logs.env:4317")],
        )
        .otlp_endpoint_for_all("http://collector:4317");

        for signal in [Signal::Traces, Signal::Metrics, Signal::Logs] {
            let target = config.otlp_target_for(signal).expect("target");
            assert_eq!(target.endpoint, "http://collector:4317", "{signal:?}");
            assert!(!target.exact, "{signal:?}");
        }
    }

    #[cfg(all(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn protocol_for_all_replaces_overrides_and_the_base_setter_does_not() {
        let config = overlay(
            TelemetryConfig::new("svc").otlp_logs_protocol(Protocol::HttpProtobuf),
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"),
                ("OTEL_EXPORTER_OTLP_METRICS_PROTOCOL", "http/protobuf"),
            ],
        )
        .otlp_protocol_for_all(Protocol::Grpc);

        for signal in [Signal::Traces, Signal::Metrics, Signal::Logs] {
            let target = config.otlp_target_for(signal).expect("target");
            assert_eq!(target.protocol, Protocol::Grpc, "{signal:?}");
        }

        // The plain setter only changes the default.
        let config = overlay(
            TelemetryConfig::new("svc").otlp_logs_protocol(Protocol::HttpProtobuf),
            &[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317")],
        )
        .otlp_protocol(Protocol::Grpc);
        let logs = config.otlp_target_for(Signal::Logs).expect("logs");
        assert_eq!(logs.protocol, Protocol::HttpProtobuf);
    }

    #[test]
    fn an_unsupported_protocol_is_ignored() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json")],
        );
        assert_eq!(config.otlp_protocol, None);
    }

    #[test]
    fn a_blank_endpoint_configures_no_endpoint() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_EXPORTER_OTLP_ENDPOINT", "  ")],
        );
        assert_eq!(config.otlp_endpoint, None);
    }

    #[test]
    fn the_sample_ratio_is_clamped_and_nan_is_ignored() {
        let config = TelemetryConfig::new("svc").sample_ratio(7.0);
        assert_eq!(config.sample_ratio, Some(1.0));

        let config = TelemetryConfig::new("svc")
            .sample_ratio(0.25)
            .sample_ratio(f64::NAN);
        assert_eq!(config.sample_ratio, Some(0.25));

        assert_eq!(
            TelemetryConfig::new("svc")
                .sample_ratio(f64::NAN)
                .sample_ratio,
            None
        );
    }

    #[test]
    fn sdk_disabled_turns_export_off() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_SDK_DISABLED", "TRUE")],
        );
        assert_eq!(config.exporter, Exporter::None);

        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_SDK_DISABLED", "false")],
        );
        assert_eq!(config.exporter, Exporter::Otlp);

        let config = overlay(TelemetryConfig::new("svc"), &[("OTEL_SDK_DISABLED", "1")]);
        assert_eq!(
            config.exporter,
            Exporter::Otlp,
            "only true disables the SDK"
        );
    }

    #[test]
    fn the_filters_and_propagators_follow_call_order_precedence() {
        // The overlay reads RUST_LOG, TELEMETRY_TRACE_FILTER and
        // OTEL_PROPAGATORS like any other variable...
        let config = overlay(
            TelemetryConfig::new("svc")
                .log_filter("info")
                .trace_filter("info")
                .propagators("none"),
            &[
                ("RUST_LOG", "debug"),
                ("TELEMETRY_TRACE_FILTER", "warn"),
                ("OTEL_PROPAGATORS", "baggage"),
            ],
        );
        assert_eq!(config.log_filter.as_deref(), Some("debug"));
        assert_eq!(config.trace_filter.as_deref(), Some("warn"));
        assert_eq!(config.propagators.as_deref(), Some("baggage"));

        // ...so a setter after the overlay wins.
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("RUST_LOG", "debug"), ("OTEL_PROPAGATORS", "baggage")],
        )
        .log_filter("info")
        .propagators("tracecontext");
        assert_eq!(config.log_filter.as_deref(), Some("info"));
        assert_eq!(config.propagators.as_deref(), Some("tracecontext"));
    }

    #[test]
    fn strict_is_read_from_the_environment() {
        let config = overlay(TelemetryConfig::new("svc"), &[("TELEMETRY_STRICT", "true")]);
        assert!(config.strict);

        let config = overlay(
            TelemetryConfig::new("svc").strict(true),
            &[("TELEMETRY_STRICT", "false")],
        );
        assert!(!config.strict);
    }

    #[test]
    fn log_format_and_trust_are_read() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[
                ("TELEMETRY_LOG_FORMAT", "Pretty"),
                ("TELEMETRY_TRUST_INCOMING_TRACE", "true"),
            ],
        );
        assert_eq!(config.log_format, LogFormat::Pretty);
        assert!(config.trust_incoming_trace);

        let config = overlay(
            TelemetryConfig::new("svc").log_format(LogFormat::Pretty),
            &[("TELEMETRY_LOG_FORMAT", "yaml")],
        );
        assert_eq!(config.log_format, LogFormat::Pretty);
    }

    #[test]
    fn the_resource_carries_only_what_was_configured() {
        let resource = TelemetryConfig::new("svc").resource();
        assert_eq!(resource.schema_url(), Some(SCHEMA_URL));
        assert_eq!(attribute(&resource, SERVICE_NAME).as_deref(), Some("svc"));
        assert_eq!(attribute(&resource, SERVICE_VERSION), None);
        assert_eq!(attribute(&resource, DEPLOYMENT_ENVIRONMENT_NAME), None);

        let resource = TelemetryConfig::new("svc")
            .service_version("1.2.3")
            .environment("staging")
            .resource_attribute("cloud.region", "eu-west-1")
            .resource();
        assert_eq!(
            attribute(&resource, SERVICE_VERSION).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            attribute(&resource, DEPLOYMENT_ENVIRONMENT_NAME).as_deref(),
            Some("staging")
        );
        assert_eq!(
            attribute(&resource, "cloud.region").as_deref(),
            Some("eu-west-1")
        );
    }
}
