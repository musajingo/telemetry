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
- **Boots with or without a collector.** With no endpoint configured the
  service just logs to stdout. An exporter that cannot be built disables that
  one signal instead of taking the service down.
- **`OtelGuard`** shuts the exporters down on drop, flushes on demand for
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
initialized inside a Tokio runtime. OTLP over HTTP is available too and needs
no async runtime. See [Transports](#transports).

Requires Rust 1.98 or newer.

## Quick start

```rust,no_run
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reads the environment. The name is used unless OTEL_SERVICE_NAME is set.
    let guard = telemetry::init_telemetry(env!("CARGO_PKG_NAME"));

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

`init_telemetry(name)` is shorthand for
`TelemetryConfig::from_env(name).init()`. Use `TelemetryConfig` directly to
set things in code:

```rust,no_run
use std::time::Duration;
use telemetry::{LogFormat, Protocol, TelemetryConfig};

# #[tokio::main]
# async fn main() -> Result<(), telemetry::InitError> {
let guard = TelemetryConfig::new(env!("CARGO_PKG_NAME"))
    .service_version(env!("CARGO_PKG_VERSION"))
    .environment("staging")
    // Everything above is a fallback the environment can override...
    .with_env()
    // ...and everything below always wins.
    .otlp_endpoint("http://collector.internal:4317")
    .otlp_protocol(Protocol::Grpc)
    .sample_ratio(0.1)
    .metrics_interval(Duration::from_secs(30))
    .log_format(LogFormat::Json)
    .log_filter("info,sqlx=warn")
    .resource_attribute("cloud.region", "eu-west-1")
    .try_init()?;
# drop(guard);
# Ok(())
# }
```

Precedence is call order. `with_env()` overlays whatever is set in the
environment on top of the values so far, so setters before it are fallbacks and
setters after it are overrides. `TelemetryConfig::new` alone reads none of the
variables below.

`init()` panics where `try_init()` returns an `InitError`: a global `tracing`
subscriber is already installed, or the log filter does not parse. A failed
init leaves all global state untouched.

A library cannot see the name or version of the binary it is linked into, which
is why both are passed in. `service.version` and `deployment.environment.name`
are omitted when not configured.

### Environment variables

Read by `with_env()` / `from_env()`:

| Variable                         | Meaning                                                                                                                               |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `OTEL_SERVICE_NAME`              | `service.name`                                                                                                                        |
| `OTEL_SERVICE_VERSION`           | `service.version`                                                                                                                     |
| `ENVIRONMENT`                    | `deployment.environment.name`                                                                                                         |
| `OTEL_EXPORTER_OTLP_ENDPOINT`    | Collector base URL, e.g. `http://localhost:4317`                                                                                      |
| `OTEL_HOST`, `OTEL_PORT`         | Builds `http://{host}:{port}` when the endpoint above is unset. The port defaults to the protocol's: `4317` for gRPC, `4318` for HTTP |
| `OTEL_EXPORTER_OTLP_PROTOCOL`    | `grpc` or `http/protobuf`. See [Transports](#transports)                                                                              |
| `OTEL_SDK_DISABLED`              | `true` turns all export off                                                                                                           |
| `OTEL_TRACES_SAMPLE_RATIO`       | Fraction of new traces to sample, `0.0` to `1.0`                                                                                      |
| `TELEMETRY_LOG_FORMAT`           | `text` (default), `pretty` or `json`                                                                                                  |
| `TELEMETRY_TRUST_INCOMING_TRACE` | `true` continues inbound `traceparent` headers. See [HTTP servers](#http-servers)                                                     |

Read during init, whenever the matching setting was left unset:

| Variable                                         | Meaning                                                     |
| ------------------------------------------------ | ----------------------------------------------------------- |
| `RUST_LOG`                                       | Log filter. Defaults to `info`                              |
| `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG` | Standard sampler selection. Defaults to sampling everything |
| `OTEL_METRIC_EXPORT_INTERVAL`                    | Milliseconds between metric exports. Defaults to `60000`    |
| `OTEL_RESOURCE_ATTRIBUTES`                       | Extra resource attributes, `key=value,key=value`            |

The last three are read by the OpenTelemetry SDK itself, along with the batch
processor tuning variables (`OTEL_BSP_*`, `OTEL_BLRP_*`) and the OTLP exporter's
`OTEL_EXPORTER_OTLP_HEADERS`, `_TIMEOUT` and `_COMPRESSION`.

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

`OtelMakeSpan` and `OtelOnResponse` plug into tower-http's `TraceLayer`, which
works with axum, tonic and any other tower service:

```rust
use telemetry::{OtelMakeSpan, OtelOnResponse};
use tower_http::trace::{DefaultOnResponse, TraceLayer};

let trace_layer = TraceLayer::new_for_http()
    .make_span_with(OtelMakeSpan)
    .on_response(OtelOnResponse::new(
        DefaultOnResponse::new().level(tracing::Level::INFO),
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
`TelemetryConfig::trust_incoming_trace(true)` or
`TELEMETRY_TRUST_INCOMING_TRACE=true`. Whoever controls the inbound
`traceparent` controls your sampling decision (and so your bill) and your trace
identity. Enable it only where every caller is trusted, or where a proxy in
front of the service strips trace headers sent by clients.

The crate is built against tower-http 0.6, the version axum 0.8 uses. The
traits only line up when your application is on the same tower-http version.

## Outbound propagation

HTTP clients usually have a middleware for this. For everything else,
`current_trace_headers()` returns the W3C pairs for the current span:

```rust
let mut metadata: Vec<(String, String)> = Vec::new();

for (key, value) in telemetry::current_trace_headers() {
    // e.g. insert into gRPC metadata or a queue message's attributes
    metadata.push((key, value));
}
```

It is empty when there is no active trace, so it is always safe to call.

## Short-lived and frozen processes

Batching exporters only help if the batch gets sent. AWS Lambda freezes the
sandbox right after the response and later kills it without a `SIGTERM`, so
neither a background flush nor `Drop` can be relied on. Flush at the end of
every invocation:

```rust,no_run
# #[tokio::main]
# async fn main() {
let guard = telemetry::init_telemetry("my-function");

// ...in the handler, once the response is ready:
guard.force_flush();
# }
```

## Panics

Panics do not go through `tracing`. The default hook writes to stderr, so a
panic never reaches the log exporter, and a panic inside a spawned task kills
only that task while the process carries on with no record of it.

Init installs a panic hook that first emits an `ERROR` event with the message,
location, thread name and (when `RUST_BACKTRACE` is set) the backtrace, then
calls the previous hook, so the usual stderr output is unchanged. Opt out with
`TelemetryConfig::panic_hook(false)`.

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
    let _guard = telemetry::init_testing_telemetry("my-service");
    // Spans and metrics produced from here on are printed to stdout.
}
```

The `tracing` subscriber is global, so this works once per test binary. Where
several tests may race to initialize it, use
`TelemetryConfig::new(..).exporter(Exporter::Stdout).try_init()` and ignore
`InitError::AlreadyInitialized`.

## Feature flags

| Feature         | Default | Provides                                                                                         |
| --------------- | ------- | ------------------------------------------------------------------------------------------------ |
| `otlp-grpc`     | yes     | `Protocol::Grpc` (pulls in `tonic`)                                                              |
| `otlp-http`     | no      | `Protocol::HttpProtobuf` (pulls in `reqwest`)                                                    |
| `otlp-grpc-tls` | no      | `https://` endpoints over gRPC, using ring and the platform's root store                         |
| `otlp-http-tls` | no      | `https://` endpoints over HTTP, using rustls (aws-lc-rs) and the platform's certificate verifier |
| `tower-http`    | yes     | `OtelMakeSpan` and `OtelOnResponse` (pulls in `tower-http` and `http`)                           |
| `json`          | yes     | `LogFormat::Json`                                                                                |
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
