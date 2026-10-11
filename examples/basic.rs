//! Traces, logs and a metric from a tiny program.
//!
//! Start a collector, then point the example at it:
//!
//! ```sh
//! OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 cargo run --example basic
//! ```
//!
//! Or over HTTP:
//!
//! ```sh
//! OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
//! OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
//!     cargo run --example basic --features otlp-http
//! ```
//!
//! Without an endpoint it only logs to stdout.

use telemetry::TelemetryConfig;
use telemetry::opentelemetry::{KeyValue, global};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let guard = TelemetryConfig::new("basic-example")
        .service_version(env!("CARGO_PKG_VERSION"))
        .environment("local")
        .with_env()
        .try_init()?;

    let result = run().await;

    guard.report_exit(&result);
    result
}

#[tracing::instrument]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let orders = global::meter("basic-example")
        .u64_counter("orders_processed")
        .build();

    for order_id in 1..=3 {
        process(order_id).await;
        orders.add(1, &[KeyValue::new("kind", "example")]);
    }

    tracing::info!("all orders processed");
    Ok(())
}

#[tracing::instrument]
async fn process(order_id: u32) {
    tracing::info!(order_id, "processing order");
}
