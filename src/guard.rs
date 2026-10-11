//! The RAII guard that flushes and shuts down the telemetry providers.

use opentelemetry_sdk::{
    logs::SdkLoggerProvider, metrics::SdkMeterProvider, trace::SdkTracerProvider,
};

/// Owns the telemetry providers and shuts them down when dropped.
///
/// Hold it for the whole life of the process, typically as a local in `main`.
/// Dropping it flushes whatever is still batched and stops the exporters.
#[must_use = "telemetry stops as soon as the guard is dropped"]
pub struct OtelGuard {
    pub(crate) service: String,
    pub(crate) tracer_provider: Option<SdkTracerProvider>,
    pub(crate) meter_provider: Option<SdkMeterProvider>,
    pub(crate) logger_provider: Option<SdkLoggerProvider>,
}

impl OtelGuard {
    /// Flushes every provider's pending telemetry synchronously.
    ///
    /// Needed wherever the process can be frozen or killed without warning.
    /// AWS Lambda is the usual case: the sandbox is frozen right after the
    /// response and later killed with no `SIGTERM`, so telemetry that is
    /// still batched never leaves it, and the `Drop` flush never runs. Call
    /// this at the end of each invocation.
    pub fn force_flush(&self) {
        if let Some(tracer_provider) = &self.tracer_provider
            && let Err(err) = tracer_provider.force_flush()
        {
            eprintln!("telemetry: trace flush failed ({err:?})");
        }
        if let Some(meter_provider) = &self.meter_provider
            && let Err(err) = meter_provider.force_flush()
        {
            eprintln!("telemetry: metric flush failed ({err:?})");
        }
        if let Some(logger_provider) = &self.logger_provider
            && let Err(err) = logger_provider.force_flush()
        {
            eprintln!("telemetry: log flush failed ({err:?})");
        }
    }

    /// Reports a process-fatal `main` result through `tracing`, then flushes.
    ///
    /// When `main` returns `Err`, the runtime's `Termination` impl prints the
    /// error with a bare stderr write that never reaches the OTLP log bridge.
    /// The failure that killed the process is then the one thing missing from
    /// the telemetry backend. Call this on the way out of `main`.
    pub fn report_exit<T, E: std::fmt::Debug>(&self, result: &Result<T, E>) {
        if let Err(error) = result {
            tracing::error!(
                service = %self.service,
                error = ?error,
                "process exited with error"
            );
            self.force_flush();
        }
    }
}

// Shutdown errors are printed to stderr: by now the log pipeline is the thing
// being torn down.
impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Some(tracer_provider) = &self.tracer_provider
            && let Err(err) = tracer_provider.shutdown()
        {
            eprintln!("telemetry: tracer shutdown failed ({err:?})");
        }
        if let Some(meter_provider) = &self.meter_provider
            && let Err(err) = meter_provider.shutdown()
        {
            eprintln!("telemetry: meter shutdown failed ({err:?})");
        }
        if let Some(logger_provider) = &self.logger_provider
            && let Err(err) = logger_provider.shutdown()
        {
            eprintln!("telemetry: logger shutdown failed ({err:?})");
        }
    }
}
