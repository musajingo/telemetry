# telemetry

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

One-call [OpenTelemetry](https://opentelemetry.io) setup for Rust services.

Call it once at the top of `main`. Your `tracing` spans become distributed
traces, your log events become log records tied to those traces, and your
metrics are exported — all over OTLP, OpenTelemetry's wire protocol. The crate
also handles the parts that are easy to get wrong: flushing before the process
dies, recording panics, carrying trace context between services, and keeping
request headers and query strings out of your telemetry.

It is a thin layer over the official `opentelemetry` crates, not a
replacement. You keep instrumenting with `tracing` as usual.

> **Status:** pre-1.0. The API may change between minor versions.

## Features

- **Traces, logs and metrics over OTLP**, by gRPC or HTTP. Log records carry
  the id of the trace they happened in; JSON stdout lines do too, so log
  shippers can join logs to traces.
- **Boots with or without a collector.** No endpoint? The service runs and
  logs to stdout. A broken exporter disables that one signal, not the service.
  The guard tells you what is exporting, and strict mode fails init instead.
- **Separate volume knobs for logs and traces.** Quietening the logs never
  drops traces, and vice versa.
- **A guard for shutdown**: flushes and stops the exporters on drop, on
  demand (AWS Lambda), or fallibly via `shutdown()`.
- **Panics become `tracing` errors**, including panics in spawned tasks that
  would otherwise vanish.
- **Ready-made HTTP middleware** for tower/axum: one request span per call,
  5xx marked as failures, headers and query strings never recorded.
- **Trace context across services**: opt-in continuation of inbound traces,
  and `current_trace_headers()` for outbound gRPC, queues and the like.

## Installation

Not on crates.io yet; depend on it from GitHub, pinned to a tag:

```toml
[dependencies]
telemetry = { git = "https://github.com/musajingo/telemetry", tag = "v0.1.0" }
tracing = "0.1"
```

For the API docs, run `cargo doc --open -p telemetry` in your project.
Requires Rust 1.98 or newer.

The default transport is gRPC, whose client needs a Tokio runtime; initialize
after `#[tokio::main]` starts. OTLP over HTTP needs no runtime. See
[Transports](#transports).

## Quick start

```rust,no_run
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let guard = telemetry::init(env!("CARGO_PKG_NAME"));

    let result = run().await;

    guard.report_exit(&result);
    result
}

#[tracing::instrument]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("service started");
    Ok(())
}
```

Point it at a collector and run:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 cargo run
```

Keep the guard alive for the whole process; dropping it flushes and shuts the
exporters down. [`examples/basic.rs`](examples/basic.rs) is a runnable version
with traces, logs and a metric.

## Configuration

`telemetry::init(name)` reads the environment and uses defaults for the rest.
For code-level control, use `TelemetryConfig`:

```rust,no_run
use telemetry::{LogFormat, TelemetryConfig};

#[tokio::main]
async fn main() -> Result<(), telemetry::InitError> {
  let guard = TelemetryConfig::new(env!("CARGO_PKG_NAME"))
      .service_version(env!("CARGO_PKG_VERSION"))
      .with_env()
      .otlp_endpoint("http://collector.internal:4317")
      .log_format(LogFormat::Json)
      .try_init()?;

  Ok(())
}
```

Precedence is call order, per setting: `with_env()` overrides setters called
before it, and setters called after it override the environment.

`init()` panics where `try_init()` returns an `InitError`: a `tracing`
subscriber is already installed, a filter does not parse, or gRPC export is
configured outside a Tokio runtime. A failed init changes nothing globally.

By default a missing endpoint or a broken exporter is reported on stderr and
the service boots without that signal. The guard reports what came up
(`exports_traces()`, `exporter_failures()`, ...). If booting without telemetry
should be an error instead, set `strict(true)` or `TELEMETRY_STRICT=true`.

### Environment variables

Variables read by `with_env()` / `from_env()`. Call the matching setter
_after_ `with_env()` to override a variable from code.

| Variable                               | Meaning                                                                                           | Setter                      |
| -------------------------------------- | ------------------------------------------------------------------------------------------------- | --------------------------- |
| `OTEL_SERVICE_NAME`                    | Service name; defaults to the name passed in code                                                 | `service_name`              |
| `OTEL_EXPORTER_OTLP_ENDPOINT`          | Collector base URL with scheme and port, e.g. `http://localhost:4317`. Unset: nothing is exported | `otlp_endpoint`             |
| `OTEL_EXPORTER_OTLP_PROTOCOL`          | `grpc` (default) or `http/protobuf`                                                               | `otlp_protocol`             |
| `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT` | Per-signal URL (`TRACES`, `METRICS`, `LOGS`), used exactly as given                               | `otlp_traces_endpoint`, ... |
| `OTEL_EXPORTER_OTLP_<SIGNAL>_PROTOCOL` | Per-signal transport                                                                              | `otlp_traces_protocol`, ... |
| `OTEL_SDK_DISABLED`                    | `true` turns all export off                                                                       | `exporter`                  |
| `OTEL_PROPAGATORS`                     | `tracecontext`, `baggage` or `none`, comma-separated; default both                                | `propagators`               |
| `RUST_LOG`                             | Log filter: stdout lines and exported log records. Default `info`                                 | `log_filter`                |
| `TELEMETRY_TRACE_FILTER`               | Span and metric filter, independent of the log filter. Default `info`                             | `trace_filter`              |
| `TELEMETRY_LOG_FORMAT`                 | `text` (default), `pretty` or `json`                                                              | `log_format`                |
| `TELEMETRY_TRUST_INCOMING_TRACE`       | `true` continues inbound traces. Default `false`                                                  | `trust_incoming_trace`      |
| `TELEMETRY_STRICT`                     | `true` fails init when OTLP export will not happen                                                | `strict`                    |

A per-signal endpoint wins over the base URL and is used verbatim (no
`/v1/<signal>` appended); on its own it enables just that signal. The plain
`otlp_endpoint`/`otlp_protocol` setters change the shared default and keep
per-signal overrides; the `..._for_all` variants also clear them — use those
after `with_env()` when the environment must not redirect any signal.

The OpenTelemetry SDK reads its own standard variables during init, even
without `with_env()`:

| Variable                                         | Meaning                                                                                                 | Setter                                                 |
| ------------------------------------------------ | ------------------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| `OTEL_RESOURCE_ATTRIBUTES`                       | `key=value,...` attached to all telemetry; put `service.version` and `deployment.environment.name` here | `service_version`, `environment`, `resource_attribute` |
| `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG` | Which traces to sample; default samples everything                                                      | `sample_ratio`                                         |
| `OTEL_METRIC_EXPORT_INTERVAL`                    | Milliseconds between metric exports; default `60000`                                                    | `metrics_export_interval`                              |

These code setters win regardless of call order. The SDK and exporters also
read further tuning variables (batching, headers, timeouts, compression); see
the [SDK](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/)
and [OTLP exporter](https://opentelemetry.io/docs/specs/otel/protocol/exporter/)
specifications.

Earlier versions read some nonstandard variables; they are no longer read:

| Removed                    | Use instead                                                                      |
| -------------------------- | -------------------------------------------------------------------------------- |
| `OTEL_SERVICE_VERSION`     | `OTEL_RESOURCE_ATTRIBUTES=service.version=1.2.3`                                 |
| `ENVIRONMENT`              | `OTEL_RESOURCE_ATTRIBUTES=deployment.environment.name=production`                |
| `OTEL_HOST`, `OTEL_PORT`   | `OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4317`                              |
| `OTEL_TRACES_SAMPLE_RATIO` | `OTEL_TRACES_SAMPLER=parentbased_traceidratio` and `OTEL_TRACES_SAMPLER_ARG=0.1` |

## Transports

| Protocol        | Feature               | Default port | Needs Tokio |
| --------------- | --------------------- | ------------ | ----------- |
| `grpc`          | `otlp-grpc` (default) | `4317`       | yes         |
| `http/protobuf` | `otlp-http`           | `4318`       | no          |

With both compiled in, gRPC is used unless configured otherwise. With only
`otlp-http`, HTTP is the default and `tonic` is not compiled at all:

```toml
telemetry = { git = "https://github.com/musajingo/telemetry", tag = "v0.1.0", default-features = false, features = ["otlp-http", "otlp-http-tls"] }
```

Over HTTP the standard `/v1/traces`, `/v1/metrics` and `/v1/logs` paths are
appended to the base URL. Credentials for hosted backends go in the standard
`OTEL_EXPORTER_OTLP_HEADERS` variable. `https://` endpoints need TLS compiled
in: `otlp-grpc-tls` or `otlp-http-tls`.

## HTTP servers

`http_trace_layer()` is a ready-made tower-http `TraceLayer` for axum, tonic
and any other tower service:

```rust,ignore
let app = axum::Router::new().layer(telemetry::http_trace_layer());
```

Each request gets a `request` span with the method, path and status; a 5xx
marks the span as failed. Headers and query strings are never recorded — that
is where tokens, cookies and emails live.

It is assembled from `OtelMakeSpan` and `OtelOnResponse`, which plug into
tower-http directly when you want different parts:

```rust
use telemetry::{OtelMakeSpan, OtelOnResponse};
use tower_http::trace::{DefaultOnResponse, TraceLayer};

let trace_layer = TraceLayer::new_for_http()
    .make_span_with(OtelMakeSpan::default())
    .on_response(OtelOnResponse::new(
        DefaultOnResponse::new().level(tracing::Level::DEBUG),
    ));
# let _ = trace_layer;
```

Joining a trace started by the caller is off by default: whoever controls the
inbound trace header controls your sampling (and so your bill) and your trace
identity. Where callers are trusted, opt in with
`TelemetryConfig::trust_incoming_trace(true)`,
`TELEMETRY_TRUST_INCOMING_TRACE=true`, or per layer with
`OtelMakeSpan::default().trust_incoming_trace(true)`.

The crate is built against tower-http 0.6, the version axum 0.8 uses.

## Outbound propagation

HTTP clients usually have middleware for this. For anything else — gRPC
metadata, queue messages — attach the headers yourself:

```rust
for (key, value) in telemetry::current_trace_headers() {
    // insert into gRPC metadata, a message's attributes, ...
    # let _ = (key, value);
}
```

It returns the W3C `traceparent`/`tracestate` pairs (and `baggage`, if any)
for the current span, and is empty when there is no active trace.

Baggage is forwarded to every downstream hop, so never put secrets in it.

## Short-lived and frozen processes

AWS Lambda freezes the process right after the response, so batched telemetry
may never leave it and `Drop` may never run. Flush at the end of every
invocation:

```rust,no_run
# #[tokio::main]
# async fn main() {
let guard = telemetry::init("my-function");

// ...at the end of each invocation:
if let Err(err) = guard.force_flush() {
    eprintln!("telemetry flush failed: {err}");
}
# }
```

## Panics

Rust panics normally go only to stderr, so they never reach your telemetry
backend — and a panic in a spawned task kills just that task, silently. Init
installs a panic hook that first emits a `tracing` error with the message,
location, thread name and (when Rust's backtrace capture is enabled via
`RUST_BACKTRACE` or `RUST_LIB_BACKTRACE`) the backtrace, then calls the
previous hook. Opt out with `TelemetryConfig::capture_panics(false)`.

## Testing and local runs

With the `stdout` feature, spans and metrics are printed instead of exported,
and `init_testing_telemetry` is a one-call preset for tests:

```toml
[dev-dependencies]
telemetry = { git = "https://github.com/musajingo/telemetry", tag = "v0.1.0", features = ["stdout"] }
```

```rust,ignore
#[tokio::test]
async fn my_test() {
    telemetry::init_testing_telemetry("my-service");
    // Spans and metrics from here on are printed to stdout.
}
```

Setup happens once per test binary and the guard is shared: every call
returns the same `&'static TelemetryGuard`, tests can run in any order, and
no test can shut telemetry down for the others.

## Feature flags

| Feature         | Default | Provides                                                         |
| --------------- | ------- | ---------------------------------------------------------------- |
| `otlp-grpc`     | yes     | `Protocol::Grpc` (pulls in `tonic`)                              |
| `otlp-http`     | no      | `Protocol::HttpProtobuf` (pulls in `reqwest`)                    |
| `otlp-grpc-tls` | no      | `https://` endpoints over gRPC                                   |
| `otlp-http-tls` | no      | `https://` endpoints over HTTP                                   |
| `tower-http`    | yes     | `http_trace_layer`, `OtelMakeSpan`, `OtelOnResponse`             |
| `json`          | yes     | `LogFormat::Json`, with `trace_id`/`span_id` on lines in a trace |
| `stdout`        | no      | `Exporter::Stdout` and `init_testing_telemetry`                  |

With no `otlp-*` feature at all you still get the log layer, the panic hook
and the guard, and none of the OTLP stack is compiled.

## Version compatibility

OpenTelemetry's globals only connect code compiled against the same
`opentelemetry` version. The crate therefore re-exports its pinned
`opentelemetry`, `opentelemetry_sdk` and `tracing_opentelemetry`: use
`telemetry::opentelemetry` instead of declaring your own copy and the
versions cannot drift apart.

```rust,no_run
use telemetry::opentelemetry::global;

let meter = global::meter("checkout");
```

| telemetry | opentelemetry | tracing-opentelemetry | tower-http |
| --------- | ------------- | --------------------- | ---------- |
| 0.1       | 0.32          | 0.33                  | 0.6        |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
