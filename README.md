# telemetry

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

One-call [OpenTelemetry](https://opentelemetry.io) setup for Rust services
built on [`tracing`](https://github.com/tokio-rs/tracing).

Call it once at the top of `main` and your `tracing` spans become distributed
traces, your events become correlated log records, and your metrics are
exported, all over OTLP. It also covers the parts that are easy to get wrong:
flushing before the process dies, panics that never reach the backend, trace
context crossing service boundaries, and secrets leaking into spans.

It is a thin layer over the official `opentelemetry` crates, not a replacement
for them. You keep instrumenting with `tracing` and `opentelemetry::global` as
usual.

> **Status:** pre-1.0. The API may change between minor versions.

## Features

- **All three signals over OTLP**, by gRPC or HTTP: traces from `tracing` spans, log
  records from `tracing` events (carrying the trace and span id of the span
  they were emitted in), and metrics from both `opentelemetry::global::meter`
  and `tracing` fields.
- **JSON log lines carry `trace_id` and `span_id`** whenever they are emitted
  inside a trace, so logs scraped from stdout by a shipper can be joined to
  the trace they belong to without going through OTLP.
- **Boots with or without a collector.** With no endpoint configured the
  service just logs to stdout. An exporter that cannot be built disables that
  one signal instead of taking the service down. The guard reports what is
  actually exporting, and a strict mode fails init instead of degrading.
- **`TelemetryGuard`** shuts the exporters down on drop (or fallibly, via
  `shutdown()`), flushes on demand for
  runtimes that freeze the process (AWS Lambda), and reports the error that
  ended `main`.
- **Panics become `tracing` errors**, including panics in spawned tasks, which
  otherwise kill one task silently and leave no record in the backend.
- **HTTP server spans for tower-http** that continue an upstream W3C trace only
  when you opt in, never record headers or query strings, and mark 5xx
  responses as failed spans.
- **Outbound propagation** for transports that are not HTTP: get the
  `traceparent` pairs for the current span and attach them to gRPC metadata, a
  queue message, or anything else.
- **Configured by environment, by code, or both**, with a stated precedence.

## Installation

The crate is not on crates.io. Depend on it straight from GitHub, pinned to a
release tag so builds stay reproducible:

```toml
[dependencies]
telemetry = { git = "https://github.com/musamahmoudjingo/telemetry", tag = "v0.1.0" }
tracing = "0.1"
```

Use `rev = "<commit>"` instead of `tag` to pin an unreleased commit. Cargo
records the exact commit in `Cargo.lock` either way, and `cargo update -p
telemetry` moves it forward.

For the API docs, run `cargo doc --open -p telemetry` in your project.

gRPC is the default transport. Its client is `tonic`, so telemetry must be
initialized inside a Tokio runtime; outside one, `try_init` reports the
missing runtime as a setup error. OTLP over HTTP is available too and needs
no async runtime. See [Transports](#transports).

Requires Rust 1.98 or newer.

## Quick start

```rust,no_run
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reads the environment. The name is used unless OTEL_SERVICE_NAME is set.
    let guard = telemetry::init(env!("CARGO_PKG_NAME"));

    let result = run().await;

    // If `run` failed, record why before the process exits.
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

Keep the guard alive for as long as the process runs. Dropping it flushes what
is still batched and shuts the exporters down.

[`examples/basic.rs`](examples/basic.rs) is a runnable version that produces
traces, logs and a metric.

## Configuration

`telemetry::init(name)` is shorthand for
`TelemetryConfig::from_env(name).init()`. Use `TelemetryConfig` directly to
set things in code:

```rust,no_run
use std::time::Duration;
use telemetry::{LogFormat, Protocol, TelemetryConfig};

# #[tokio::main]
# async fn main() -> Result<(), telemetry::InitError> {
let guard = TelemetryConfig::new(env!("CARGO_PKG_NAME"))
    .log_filter("warn")
    // The overlay can override the log filter above...
    .with_env()
    // ...and code settings below override their environment defaults.
    .service_version(env!("CARGO_PKG_VERSION"))
    .environment("staging")
    // _for_all also clears per-signal endpoint overrides.
    .otlp_endpoint_for_all("http://collector.internal:4317")
    .otlp_protocol(Protocol::Grpc)
    .sample_ratio(0.1)
    .metrics_export_interval(Duration::from_secs(30))
    .log_format(LogFormat::Json)
    .log_filter("info,sqlx=warn")
    .trace_filter("info")
    .resource_attribute("cloud.region", "eu-west-1")
    .try_init()?;
# drop(guard);
# Ok(())
# }
```

For the overlay variables below, precedence is call order per setting:
`with_env()` overrides earlier setters, and later setters override the
matching variable. SDK defaults are read during init; explicit resource,
sampling and metric interval settings override those defaults regardless of
where `with_env()` appears. `TelemetryConfig::new` reads no environment itself,
but initializing that config still uses SDK environment defaults.

`init()` panics where `try_init()` returns an `InitError`: a global `tracing`
subscriber is already installed, a filter does not parse, or gRPC export is
configured outside a Tokio runtime. A failed init leaves all global state
untouched.

By default a missing endpoint or an exporter that cannot be built is reported
on stderr and the service boots without that signal. The guard says what
actually came up — `exports_traces()`, `exports_metrics()`, `exports_logs()`
and `exporter_failures()` — for a health endpoint, say. Where booting without
telemetry is itself a failure, `strict(true)` (or `TELEMETRY_STRICT=true`)
turns both cases into an init error instead.

A library cannot see the name or version of the binary it is linked into,
which is why the name is passed in. Set `service.version` and
`deployment.environment.name` in `OTEL_RESOURCE_ATTRIBUTES` or with the
`service_version(...)` and `environment(...)` setters; otherwise they are
omitted.

### Environment variables

The `OTEL_*` settings use the [OpenTelemetry SDK names](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/)
and [OTLP exporter names](https://opentelemetry.io/docs/specs/otel/protocol/exporter/).
`TELEMETRY_*` settings are crate extensions without a standard equivalent;
`RUST_LOG` is a Rust logging convention.

The following variables are read by `with_env()` / `from_env()`. Unset or
blank values leave earlier settings intact. Invalid protocols and formats are
reported and ignored; invalid filter directives fail init. Unsupported
propagator names are reported during init and skipped. Call the listed setter
**after** `with_env()` to override its variable.

| Variable | Meaning and default | Code override |
| --- | --- | --- |
| `OTEL_SERVICE_NAME` | `service.name`; defaults to the constructor's name | `service_name(...)` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Collector base URL, including scheme and port; unset disables signals without their own endpoint | `otlp_endpoint(...)`; `otlp_endpoint_for_all(...)` also clears per-signal endpoints |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` or `http/protobuf`; defaults to gRPC if enabled, otherwise HTTP | `otlp_protocol(...)`; `otlp_protocol_for_all(...)` also clears per-signal protocols |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | Exact trace URL; defaults to the shared endpoint | `otlp_traces_endpoint(...)` |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Exact metric URL; defaults to the shared endpoint | `otlp_metrics_endpoint(...)` |
| `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` | Exact log URL; defaults to the shared endpoint | `otlp_logs_endpoint(...)` |
| `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL` | Trace transport; defaults to the shared protocol | `otlp_traces_protocol(...)` |
| `OTEL_EXPORTER_OTLP_METRICS_PROTOCOL` | Metric transport; defaults to the shared protocol | `otlp_metrics_protocol(...)` |
| `OTEL_EXPORTER_OTLP_LOGS_PROTOCOL` | Log transport; defaults to the shared protocol | `otlp_logs_protocol(...)` |
| `OTEL_SDK_DISABLED` | Case-insensitive `true` disables export; other values leave the exporter unchanged | `exporter(Exporter::Otlp)` or `exporter(Exporter::Stdout)` |
| `OTEL_PROPAGATORS` | Comma-separated `tracecontext`, `baggage`, `none`; defaults to both W3C propagators | `propagators(...)` |
| `RUST_LOG` | Filter for console logs and OTLP log records; defaults to `info` | `log_filter(...)` |
| `TELEMETRY_TRACE_FILTER` | Filter for spans and tracing-field metrics; defaults to `info` | `trace_filter(...)` |
| `TELEMETRY_LOG_FORMAT` | `text` (default), `pretty`, or `json` (requires the `json` feature) | `log_format(...)` |
| `TELEMETRY_TRUST_INCOMING_TRACE` | `true`/`1` continues incoming trace context; `false`/`0` (default) distrusts it | `trust_incoming_trace(...)`; an `OtelMakeSpan` override takes precedence on its own layer |
| `TELEMETRY_STRICT` | `true`/`1` requires successful OTLP setup; `false`/`0` (default) permits degradation | `strict(...)` |

Per-signal endpoints take precedence over the shared endpoint and are used
exactly as given. Over HTTP, `/v1/traces`, `/v1/metrics`, or `/v1/logs` is
appended only to the shared endpoint. A per-signal endpoint alone enables only
that signal. Plain default setters preserve per-signal overrides; the
`..._for_all` setters replace them, including those supplied by the environment.

Both filters use `RUST_LOG` directive syntax and remain independent. Custom
boolean options are case-insensitive; invalid values leave earlier settings
intact.

The SDK reads the following variables **during init** when the matching
providers or exporters are built, even when starting with `TelemetryConfig::new`
and never calling `with_env()`. Code overrides in this
table win regardless of call order. These defaults are those of the pinned
Rust SDK; variables with no crate setter can only be configured through the
environment when using this crate's provider setup.

| Variable | Meaning and default | Code override |
| --- | --- | --- |
| `OTEL_RESOURCE_ATTRIBUTES` | Comma-separated `key=value` resource attributes; unset adds none. Use `service.version` and `deployment.environment.name` here | `service_version(...)`, `environment(...)`, and `resource_attribute(...)` override their matching keys. The configured service name overrides `service.name` |
| `OTEL_TRACES_SAMPLER` | `always_on`, `always_off`, `traceidratio`, `parentbased_always_on` (default), `parentbased_always_off`, or `parentbased_traceidratio` | `sample_ratio(...)` installs a parent-based ratio sampler |
| `OTEL_TRACES_SAMPLER_ARG` | Ratio in `[0, 1]` for the two ratio samplers; defaults to `1` | `sample_ratio(...)` overrides both sampler variables |
| `OTEL_METRIC_EXPORT_INTERVAL` | Metric export interval in milliseconds; defaults to `60000` | A nonzero `metrics_export_interval(...)`; zero preserves the SDK interval |
| `OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT` | Maximum attributes per span; defaults to `128` | No crate setter |
| `OTEL_SPAN_EVENT_COUNT_LIMIT` | Maximum events per span; defaults to `128` | No crate setter |
| `OTEL_SPAN_LINK_COUNT_LIMIT` | Maximum links per span; defaults to `128` | No crate setter |
| `OTEL_BSP_SCHEDULE_DELAY` | Batched OTLP trace export interval in milliseconds; defaults to `5000` | No crate setter |
| `OTEL_BSP_MAX_QUEUE_SIZE` | Pending trace queue capacity; defaults to `2048` | No crate setter |
| `OTEL_BSP_MAX_EXPORT_BATCH_SIZE` | Traces per export batch; defaults to `512`, capped by queue capacity | No crate setter |
| `OTEL_BLRP_SCHEDULE_DELAY` | Batched OTLP log export interval in milliseconds; defaults to `1000` | No crate setter |
| `OTEL_BLRP_MAX_QUEUE_SIZE` | Pending log queue capacity; defaults to `2048` | No crate setter |
| `OTEL_BLRP_MAX_EXPORT_BATCH_SIZE` | Log records per export batch; defaults to `512`, capped by queue capacity | No crate setter |
| `OTEL_EXPORTER_OTLP_HEADERS` | Comma-separated, percent-encoded `key=value` exporter headers; unset adds none | No crate setter; per-signal headers replace the shared headers |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | Export timeout in milliseconds; defaults to `10000` | No crate setter; per-signal timeout takes precedence |
| `OTEL_EXPORTER_OTLP_COMPRESSION` | `gzip` or SDK extension `zstd`; unset disables compression | No crate setter; per-signal compression takes precedence |
| `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE` | `cumulative` (default), `delta`, or `lowmemory` | No crate setter |

Headers, timeouts and compression also have per-signal forms:
`OTEL_EXPORTER_OTLP_TRACES_HEADERS`, `OTEL_EXPORTER_OTLP_METRICS_HEADERS`,
`OTEL_EXPORTER_OTLP_LOGS_HEADERS`,
`OTEL_EXPORTER_OTLP_TRACES_TIMEOUT`, `OTEL_EXPORTER_OTLP_METRICS_TIMEOUT`,
`OTEL_EXPORTER_OTLP_LOGS_TIMEOUT`,
`OTEL_EXPORTER_OTLP_TRACES_COMPRESSION`, `OTEL_EXPORTER_OTLP_METRICS_COMPRESSION`,
and `OTEL_EXPORTER_OTLP_LOGS_COMPRESSION`. They use the same formats as the
shared variables. Compression requires support in the exporter dependency;
without it, exporter setup fails. This SDK does not accept the standard
compression value `none`; leave the variable unset to disable compression.

The SDK also parses `OTEL_BSP_EXPORT_TIMEOUT` (milliseconds, default `30000`)
and the Rust SDK extension `OTEL_BSP_MAX_CONCURRENT_EXPORTS` (default `1`), but
the thread-based processor used here does not apply them. Configure export
timeouts with `OTEL_EXPORTER_OTLP_TIMEOUT` or its per-signal forms instead.
`OTEL_BLRP_EXPORT_TIMEOUT` and `OTEL_METRIC_EXPORT_TIMEOUT` are not read by the
processors used here. Other OpenTelemetry variables not listed here are not
supported by this crate's setup.

Rust's backtrace capture also consults `RUST_BACKTRACE` and `RUST_LIB_BACKTRACE`
when a panic occurs. `RUST_LIB_BACKTRACE` takes precedence for the backtrace
attached to the tracing event; `0` disables capture, other values enable it,
and unset defaults to disabled. `capture_panics(false)` disables this crate's
panic reporting. These are [Rust runtime variables](https://doc.rust-lang.org/std/backtrace/index.html),
not OpenTelemetry settings.

The previous nonstandard aliases are no longer read:

| Removed variable | Standard replacement |
| --- | --- |
| `OTEL_SERVICE_VERSION` | `OTEL_RESOURCE_ATTRIBUTES=service.version=1.2.3` |
| `ENVIRONMENT` | `OTEL_RESOURCE_ATTRIBUTES=deployment.environment.name=production` |
| `OTEL_HOST`, `OTEL_PORT` | `OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4317` |
| `OTEL_TRACES_SAMPLE_RATIO` | `OTEL_TRACES_SAMPLER=parentbased_traceidratio` plus `OTEL_TRACES_SAMPLER_ARG=0.1` |

For example, standard resource and sampling configuration is:

```sh
export OTEL_RESOURCE_ATTRIBUTES='service.version=1.2.3,deployment.environment.name=production'
export OTEL_TRACES_SAMPLER=parentbased_traceidratio
export OTEL_TRACES_SAMPLER_ARG=0.1
export OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4317
```

## Transports

| Protocol        | Feature               | Default port | Needs Tokio |
| --------------- | --------------------- | ------------ | ----------- |
| `grpc`          | `otlp-grpc` (default) | `4317`       | yes         |
| `http/protobuf` | `otlp-http`           | `4318`       | no          |

Enable one or both. With both compiled in, gRPC is used unless
`TelemetryConfig::otlp_protocol` or `OTEL_EXPORTER_OTLP_PROTOCOL` says
otherwise, so one binary can talk gRPC to a local collector and HTTP to a
hosted backend. With only `otlp-http` enabled, HTTP is the default and `tonic`
is not compiled at all:

```toml
telemetry = { git = "https://github.com/musamahmoudjingo/telemetry", tag = "v0.1.0", default-features = false, features = ["otlp-http", "otlp-http-tls"] }
```

The endpoint is always the base URL. Over HTTP the standard `/v1/traces`,
`/v1/metrics` and `/v1/logs` paths are appended to it, so a gateway such as
`https://otlp.example.com/otlp` works as given. Credentials for hosted backends
go in the standard `OTEL_EXPORTER_OTLP_HEADERS` variable.

The HTTP transport uses a blocking `reqwest` client on the exporters' own
threads. That is what lets it work without a runtime, and it is safe to use
from inside one. `http/json` is not supported.

`https://` endpoints need TLS compiled in: `otlp-grpc-tls` or `otlp-http-tls`.

## HTTP servers

`http_trace_layer()` is a ready-made tower-http `TraceLayer`, which works
with axum, tonic and any other tower service:

```rust,ignore
let app = axum::Router::new().layer(telemetry::http_trace_layer());
```

It is built from `OtelMakeSpan` and `OtelOnResponse`, which plug into
tower-http's `TraceLayer` directly when you want different parts:

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

Each request gets a `request` span with the method, path and HTTP version. The
response status is recorded on it, and a 5xx marks the span as an error so the
tracing backend shows the request as failed.

Two deliberate omissions: headers and the query string are never recorded.
Headers carry bearer tokens and session cookies, and query strings are where
emails, tokens and signatures end up. Neither belongs in logs or a trace store.

Continuing an upstream trace is opt-in, through
`TelemetryConfig::trust_incoming_trace(true)`,
`TELEMETRY_TRUST_INCOMING_TRACE=true`, or for one layer only,
`OtelMakeSpan::default().trust_incoming_trace(true)`. Whoever controls the inbound
`traceparent` controls your sampling decision (and so your bill) and your trace
identity. Enable it only where every caller is trusted, or where a proxy in
front of the service strips trace headers sent by clients.

The crate is built against tower-http 0.6, the version axum 0.8 uses. The
traits only line up when your application is on the same tower-http version.

## Outbound propagation

HTTP clients usually have a middleware for this. For everything else,
`current_trace_headers()` returns the W3C pairs for the current span —
`traceparent`/`tracestate` and, when the context carries any, `baggage`:

```rust
let mut metadata: Vec<(String, String)> = Vec::new();

for (key, value) in telemetry::current_trace_headers() {
    // e.g. insert into gRPC metadata or a queue message's attributes
    metadata.push((key, value));
}
```

It is empty when there is no active trace, so it is always safe to call.

Baggage is forwarded as-is to every downstream hop (and, depending on the
backend, recorded), so never put a secret or a token in it. Inbound baggage is
only picked up where inbound trace context is trusted, and
`OTEL_PROPAGATORS=tracecontext` stops baggage from being carried at all.

## Short-lived and frozen processes

Batching exporters only help if the batch gets sent. AWS Lambda freezes the
sandbox right after the response and later kills it without a `SIGTERM`, so
neither a background flush nor `Drop` can be relied on. Flush at the end of
every invocation:

```rust,no_run
# #[tokio::main]
# async fn main() {
let guard = telemetry::init("my-function");

// ...in the handler, once the response is ready:
if let Err(err) = guard.force_flush() {
    eprintln!("telemetry flush failed: {err}");
}
# }
```

## Panics

Panics do not go through `tracing`. The default hook writes to stderr, so a
panic never reaches the log exporter, and a panic inside a spawned task kills
only that task while the process carries on with no record of it.

Init installs a panic hook that first emits an `ERROR` event with the message,
location, thread name and (when Rust backtrace capture is enabled) the backtrace, then
calls the previous hook, so the usual stderr output is unchanged. Opt out with
`TelemetryConfig::capture_panics(false)`.

## Testing and local runs

With the `stdout` feature, `Exporter::Stdout` prints spans and metrics instead
of sending them anywhere, and `init_testing_telemetry` is a preset for tests:

```toml
[dev-dependencies]
telemetry = { git = "https://github.com/musamahmoudjingo/telemetry", tag = "v0.1.0", features = ["stdout"] }
```

```rust,ignore
#[tokio::test]
async fn my_test() {
    telemetry::init_testing_telemetry("my-service");
    // Spans and metrics produced from here on are printed to stdout.
}
```

The `tracing` subscriber is global, so the setup happens once per test binary
and the guard is shared: every call returns the same `&'static TelemetryGuard`
(the first caller's service name wins), and telemetry stays up until the
process ends. Tests can call it in any order, and none of them can shut
telemetry down for the others.

## Feature flags

| Feature         | Default | Provides                                                                                         |
| --------------- | ------- | ------------------------------------------------------------------------------------------------ |
| `otlp-grpc`     | yes     | `Protocol::Grpc` (pulls in `tonic`)                                                              |
| `otlp-http`     | no      | `Protocol::HttpProtobuf` (pulls in `reqwest`)                                                    |
| `otlp-grpc-tls` | no      | `https://` endpoints over gRPC, using ring and the platform's root store                         |
| `otlp-http-tls` | no      | `https://` endpoints over HTTP, using rustls (aws-lc-rs) and the platform's certificate verifier |
| `tower-http`    | yes     | `OtelMakeSpan` and `OtelOnResponse` (pulls in `tower-http` and `http`)                           |
| `json`          | yes     | `LogFormat::Json`, with `trace_id`/`span_id` on lines in a trace (pulls in `json-subscriber`)    |
| `stdout`        | no      | `Exporter::Stdout` and `init_testing_telemetry`                                                  |

A worker with no HTTP server can drop the tower-http stack:

```toml
telemetry = { git = "https://github.com/musamahmoudjingo/telemetry", tag = "v0.1.0", default-features = false, features = ["otlp-grpc", "json"] }
```

With no `otlp-*` feature at all the crate still gives you the log layer, the
panic hook and the guard, and compiles none of the OTLP stack.

For a different TLS setup, such as aws-lc or bundled webpki roots, leave the
`*-tls` features off and enable the matching `opentelemetry-otlp` feature from
your own `Cargo.toml`. Cargo unifies the features.

## Version compatibility

OpenTelemetry's global providers only connect code that is compiled against the
same `opentelemetry` version. The crate therefore re-exports the pinned
`opentelemetry`, `opentelemetry_sdk` and `tracing_opentelemetry` crates: use
`telemetry::opentelemetry` (for meters, say) instead of declaring your own
copy, and a version mismatch becomes impossible.

```rust,no_run
use telemetry::opentelemetry::global;

let meter = global::meter("checkout");
```

If another dependency uses `opentelemetry` directly, keep it on the version
below.

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
