//! The config builder: defaults, builder-style setters and the environment
//! overlay.

use std::time::Duration;

use opentelemetry::{Key, KeyValue, Value};
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::{
    SCHEMA_URL,
    attribute::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_NAME, SERVICE_VERSION},
};

use super::endpoint::Endpoint;
use super::exporter::Exporter;
use super::log_format::LogFormat;
use super::protocol::Protocol;
use super::signal::{Signal, SignalOverride, SignalTarget};
use crate::error::InitError;
use crate::guard::OtelGuard;

/// Everything [`try_init`](Self::try_init) needs to set telemetry up.
///
/// Start from [`new`](Self::new) for pure defaults or [`from_env`](Self::from_env)
/// to also read the environment, then adjust with the setters. Precedence is
/// simply call order: a setter called after [`with_env`](Self::with_env)
/// overrides the environment, one called before it acts as a fallback.
///
/// ```
/// use telemetry::{LogFormat, TelemetryConfig};
///
/// let config = TelemetryConfig::new(env!("CARGO_PKG_NAME"))
///     .service_version(env!("CARGO_PKG_VERSION"))
///     // Anything set above can still be overridden by the environment...
///     .with_env()
///     // ...and anything set below always wins.
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
    pub(crate) otlp_endpoint: Option<Endpoint>,
    pub(crate) otlp_protocol: Option<Protocol>,
    pub(crate) otlp_traces: SignalOverride,
    pub(crate) otlp_metrics: SignalOverride,
    pub(crate) otlp_logs: SignalOverride,
    pub(crate) sample_ratio: Option<f64>,
    pub(crate) metrics_interval: Option<Duration>,
    pub(crate) log_filter: Option<String>,
    pub(crate) log_format: LogFormat,
    pub(crate) trust_incoming_trace: bool,
    pub(crate) panic_hook: bool,
}

impl TelemetryConfig {
    /// A config with the given service name and default settings. Reads no
    /// environment variables.
    ///
    /// The defaults: OTLP export (inactive until an endpoint is set) over gRPC
    /// when the `otlp-grpc` feature is on, the SDK's sampler and metric interval, text logs filtered by `RUST_LOG`
    /// (or `info`), inbound trace context not trusted, panic hook installed.
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
            metrics_interval: None,
            log_filter: None,
            log_format: LogFormat::default(),
            trust_incoming_trace: false,
            panic_hook: true,
        }
    }

    /// Shorthand for `TelemetryConfig::new(service_name).with_env()`: the
    /// given name is used unless `OTEL_SERVICE_NAME` is set.
    pub fn from_env(service_name: impl Into<String>) -> Self {
        Self::new(service_name).with_env()
    }

    /// Overlays every setting that is present in the environment on top of
    /// the current values. Unset, blank and unparsable variables leave the
    /// current value alone.
    ///
    /// | Variable | Setting |
    /// | --- | --- |
    /// | `OTEL_SERVICE_NAME` | [`service_name`](Self::service_name) |
    /// | `OTEL_SERVICE_VERSION` | [`service_version`](Self::service_version) |
    /// | `ENVIRONMENT` | [`environment`](Self::environment) |
    /// | `OTEL_EXPORTER_OTLP_ENDPOINT` | [`otlp_endpoint`](Self::otlp_endpoint) |
    /// | `OTEL_HOST`, `OTEL_PORT` | endpoint `http://{host}:{port}`, when the variable above is unset. The port defaults to the protocol's: `4317` for gRPC, `4318` for HTTP |
    /// | `OTEL_EXPORTER_OTLP_PROTOCOL` | [`otlp_protocol`](Self::otlp_protocol): `grpc` or `http/protobuf` |
    /// | `OTEL_SDK_DISABLED=true` | [`Exporter::None`] |
    /// | `OTEL_TRACES_SAMPLE_RATIO` | [`sample_ratio`](Self::sample_ratio) |
    /// | `TELEMETRY_LOG_FORMAT` | [`log_format`](Self::log_format): `text`, `pretty` or `json` |
    /// | `TELEMETRY_TRUST_INCOMING_TRACE` | [`trust_incoming_trace`](Self::trust_incoming_trace) |
    ///
    /// `RUST_LOG` and the OpenTelemetry SDK's own variables are not part of
    /// the overlay; they are read during init whenever the matching setting
    /// was left unset. See the crate documentation for the list.
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
        if let Some(version) = var("OTEL_SERVICE_VERSION") {
            self.service_version = Some(version);
        }
        if let Some(environment) = var("ENVIRONMENT") {
            self.environment = Some(environment);
        }

        if let Some(endpoint) = var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            self.otlp_endpoint = Some(Endpoint::Url(endpoint));
        } else if let Some(host) = var("OTEL_HOST") {
            self.otlp_endpoint = Some(Endpoint::HostPort {
                host,
                port: var("OTEL_PORT"),
            });
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
        if var("OTEL_SDK_DISABLED").and_then(|v| parse_bool(&v)) == Some(true) {
            self.exporter = Exporter::None;
        }

        if let Some(ratio) = var("OTEL_TRACES_SAMPLE_RATIO").and_then(|v| v.parse::<f64>().ok()) {
            self = self.sample_ratio(ratio);
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

        self
    }

    /// Sets `service.name`. It is also used as the tracer (instrumentation
    /// scope) name.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Sets `service.version`. A library cannot see the version of the binary
    /// it is linked into, so pass `env!("CARGO_PKG_VERSION")` from the
    /// application. Left unset, the attribute is omitted.
    pub fn service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Sets `deployment.environment.name`, e.g. `production` or `staging`.
    /// Left unset, the attribute is omitted.
    pub fn environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = Some(environment.into());
        self
    }

    /// Adds one more resource attribute, e.g. `cloud.region`.
    pub fn resource_attribute(mut self, key: impl Into<Key>, value: impl Into<Value>) -> Self {
        self.resource_attributes.push(KeyValue::new(key, value));
        self
    }

    /// Chooses where telemetry is exported. Defaults to [`Exporter::Otlp`].
    pub fn exporter(mut self, exporter: Exporter) -> Self {
        self.exporter = exporter;
        self
    }

    /// Sets the collector endpoint, e.g. `http://localhost:4317`.
    ///
    /// This is the base URL for all three signals. Over HTTP the standard
    /// `/v1/traces`, `/v1/metrics` and `/v1/logs` paths are appended to it. An
    /// `https://` endpoint needs the `otlp-grpc-tls` or `otlp-http-tls`
    /// feature.
    ///
    /// Called after [`with_env`](Self::with_env), this also discards any
    /// per-signal `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT` the overlay read:
    /// precedence is call order, and a later setter wins over the whole
    /// environment, not just its own variable.
    pub fn otlp_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(Endpoint::Url(endpoint.into()));
        self.otlp_traces.endpoint = None;
        self.otlp_metrics.endpoint = None;
        self.otlp_logs.endpoint = None;
        self
    }

    /// Chooses the OTLP transport. Defaults to gRPC when the `otlp-grpc`
    /// feature is enabled and to HTTP otherwise.
    ///
    /// Called after [`with_env`](Self::with_env), this also discards any
    /// per-signal `OTEL_EXPORTER_OTLP_<SIGNAL>_PROTOCOL` the overlay read,
    /// for the same reason as [`otlp_endpoint`](Self::otlp_endpoint).
    pub fn otlp_protocol(mut self, protocol: Protocol) -> Self {
        self.otlp_protocol = Some(protocol);
        self.otlp_traces.protocol = None;
        self.otlp_metrics.protocol = None;
        self.otlp_logs.protocol = None;
        self
    }

    /// Samples this fraction of new traces, clamped to `[0.0, 1.0]`. The
    /// sampler is parent-based, so a sampled upstream trace is always
    /// continued.
    ///
    /// Left unset, the SDK default applies: `OTEL_TRACES_SAMPLER` and
    /// `OTEL_TRACES_SAMPLER_ARG` if present, otherwise every trace is sampled.
    pub fn sample_ratio(mut self, ratio: f64) -> Self {
        if !ratio.is_nan() {
            self.sample_ratio = Some(ratio.clamp(0.0, 1.0));
        }
        self
    }

    /// Sets how often metrics are exported.
    ///
    /// Left unset, the SDK default applies: `OTEL_METRIC_EXPORT_INTERVAL`
    /// (milliseconds) if present, otherwise 60 seconds.
    pub fn metrics_interval(mut self, interval: Duration) -> Self {
        self.metrics_interval = Some(interval);
        self
    }

    /// Sets the log filter, in `RUST_LOG` syntax (`info,sqlx=warn`).
    ///
    /// Left unset, `RUST_LOG` is used, falling back to `info`.
    pub fn log_filter(mut self, directives: impl Into<String>) -> Self {
        self.log_filter = Some(directives.into());
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
    pub fn panic_hook(mut self, install: bool) -> Self {
        self.panic_hook = install;
        self
    }

    /// Installs the global subscriber and returns the guard that owns the
    /// telemetry providers.
    ///
    /// # Panics
    ///
    /// Panics where [`try_init`](Self::try_init) would return an error.
    pub fn init(self) -> OtelGuard {
        match self.try_init() {
            Ok(guard) => guard,
            Err(err) => panic!("failed to initialize telemetry: {err}"),
        }
    }

    /// Installs the global subscriber and returns the guard that owns the
    /// telemetry providers.
    ///
    /// With [`Protocol::Grpc`] this must be called inside a Tokio runtime,
    /// which the gRPC client needs. An exporter that cannot be built is
    /// reported on stderr and that signal is disabled, rather than failing
    /// the whole service over its telemetry.
    ///
    /// # Errors
    ///
    /// Fails, leaving every global untouched, when a global `tracing`
    /// subscriber is already installed or the configured log filter does not
    /// parse.
    pub fn try_init(self) -> Result<OtelGuard, InitError> {
        crate::init::try_init(self)
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
            None => (self.base_endpoint(protocol)?, false),
        };
        Some(SignalTarget {
            protocol,
            endpoint,
            exact,
        })
    }

    /// The base endpoint resolved against a protocol's default port.
    #[cfg_attr(
        not(any(feature = "otlp-grpc", feature = "otlp-http")),
        allow(dead_code)
    )]
    fn base_endpoint(&self, protocol: Protocol) -> Option<String> {
        Some(match self.otlp_endpoint.as_ref()? {
            Endpoint::Url(url) => url.clone(),
            Endpoint::HostPort { host, port } => {
                let port = port.as_deref().unwrap_or(protocol.default_port());
                format!("http://{host}:{port}")
            }
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
            TelemetryConfig::new("fallback").service_version("0.0.1"),
            &[
                ("OTEL_SERVICE_NAME", "from-env"),
                ("OTEL_SERVICE_VERSION", "9.9.9"),
                ("ENVIRONMENT", "staging"),
            ],
        )
        .environment("production");

        assert_eq!(config.service_name, "from-env");
        assert_eq!(config.service_version.as_deref(), Some("9.9.9"));
        assert_eq!(config.environment.as_deref(), Some("production"));
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn the_standard_endpoint_wins_over_host_and_port() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", " http://collector:4317 "),
                ("OTEL_HOST", "ignored"),
            ],
        );
        assert_eq!(endpoint(&config).as_deref(), Some("http://collector:4317"));
    }

    #[cfg(feature = "otlp-grpc")]
    #[test]
    fn host_and_port_build_an_endpoint() {
        let config = overlay(TelemetryConfig::new("svc"), &[("OTEL_HOST", "collector")]);
        assert_eq!(endpoint(&config).as_deref(), Some("http://collector:4317"));

        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_HOST", "collector"), ("OTEL_PORT", "14317")],
        );
        assert_eq!(endpoint(&config).as_deref(), Some("http://collector:14317"));
    }

    #[cfg(feature = "otlp-http")]
    #[test]
    fn the_default_port_follows_the_protocol() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[
                ("OTEL_HOST", "collector"),
                ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
            ],
        );
        assert_eq!(
            config.otlp_target_for(Signal::Traces),
            Some(SignalTarget {
                protocol: Protocol::HttpProtobuf,
                endpoint: "http://collector:4318".to_string(),
                exact: false,
            })
        );

        // A protocol chosen after the overlay still gets its own default port.
        let config = overlay(TelemetryConfig::new("svc"), &[("OTEL_HOST", "collector")])
            .otlp_protocol(Protocol::HttpProtobuf);
        assert_eq!(endpoint(&config).as_deref(), Some("http://collector:4318"));
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
                ("OTEL_HOST", "collector"),
                ("OTEL_EXPORTER_OTLP_METRICS_PROTOCOL", "http/protobuf"),
            ],
        );

        let traces = config.otlp_target_for(Signal::Traces).expect("traces");
        assert_eq!(traces.protocol, Protocol::Grpc);
        assert_eq!(traces.endpoint, "http://collector:4317");

        // The protocol override also drives the default port.
        let metrics = config.otlp_target_for(Signal::Metrics).expect("metrics");
        assert_eq!(metrics.protocol, Protocol::HttpProtobuf);
        assert_eq!(metrics.endpoint, "http://collector:4318");
    }

    #[cfg(any(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn an_endpoint_set_after_the_overlay_wins_over_per_signal_variables() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[(
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "http://injected:4317",
            )],
        )
        .otlp_endpoint("http://collector:4317");

        let traces = config.otlp_target_for(Signal::Traces).expect("traces");
        assert_eq!(
            traces.endpoint, "http://collector:4317",
            "a setter after with_env overrides the environment, per-signal included"
        );
    }

    #[cfg(all(feature = "otlp-grpc", feature = "otlp-http"))]
    #[test]
    fn a_protocol_set_after_the_overlay_wins_over_per_signal_variables() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[
                ("OTEL_HOST", "collector"),
                ("OTEL_EXPORTER_OTLP_METRICS_PROTOCOL", "http/protobuf"),
            ],
        )
        .otlp_protocol(Protocol::Grpc);

        let metrics = config.otlp_target_for(Signal::Metrics).expect("metrics");
        assert_eq!(metrics.protocol, Protocol::Grpc);
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
    fn a_blank_host_configures_no_endpoint() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_HOST", "  "), ("OTEL_EXPORTER_OTLP_ENDPOINT", "")],
        );
        assert_eq!(config.otlp_endpoint, None);
    }

    #[test]
    fn the_sample_ratio_is_clamped_and_garbage_is_ignored() {
        let config = overlay(
            TelemetryConfig::new("svc"),
            &[("OTEL_TRACES_SAMPLE_RATIO", "7")],
        );
        assert_eq!(config.sample_ratio, Some(1.0));

        let config = overlay(
            TelemetryConfig::new("svc").sample_ratio(0.25),
            &[("OTEL_TRACES_SAMPLE_RATIO", "lots")],
        );
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
