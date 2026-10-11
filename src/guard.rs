//! The RAII guard that flushes and shuts down the telemetry providers.

use opentelemetry_sdk::{
    logs::SdkLoggerProvider, metrics::SdkMeterProvider, trace::SdkTracerProvider,
};

use crate::lifecycle_error::LifecycleError;

/// Owns the telemetry providers and shuts them down when dropped.
///
/// Hold it for the whole life of the process, typically as a local in `main`.
/// Dropping it flushes whatever is still batched and stops the exporters,
/// best-effort; call [`shutdown`](Self::shutdown) to handle failures instead.
#[must_use = "telemetry stops as soon as the guard is dropped"]
pub struct OtelGuard {
    pub(crate) service: String,
    pub(crate) tracer_provider: Option<SdkTracerProvider>,
    pub(crate) meter_provider: Option<SdkMeterProvider>,
    pub(crate) logger_provider: Option<SdkLoggerProvider>,
    pub(crate) exporter_failures: Vec<String>,
}

impl OtelGuard {
    /// Whether spans are being exported. When false there are also no trace
    /// ids on log lines and no outbound trace headers.
    pub fn exports_traces(&self) -> bool {
        self.tracer_provider.is_some()
    }

    /// Whether metrics are being exported.
    pub fn exports_metrics(&self) -> bool {
        self.meter_provider.is_some()
    }

    /// Whether log records are being exported. Stdout logging is independent
    /// of this and always on.
    pub fn exports_logs(&self) -> bool {
        self.logger_provider.is_some()
    }

    /// The exporters that could not be built, one message per failed signal.
    /// Empty when everything configured came up.
    pub fn exporter_failures(&self) -> &[String] {
        &self.exporter_failures
    }

    /// Flushes every provider's pending telemetry synchronously, reporting
    /// the signals that failed.
    ///
    /// Needed wherever the process can be frozen or killed without warning.
    /// AWS Lambda is the usual case: the sandbox is frozen right after the
    /// response and later killed with no `SIGTERM`, so telemetry that is
    /// still batched never leaves it, and the `Drop` flush never runs. Call
    /// this at the end of each invocation.
    pub fn force_flush(&self) -> Result<(), LifecycleError> {
        let mut failures = Vec::new();
        if let Some(provider) = &self.tracer_provider
            && let Err(err) = provider.force_flush()
        {
            failures.push(("traces", err));
        }
        if let Some(provider) = &self.meter_provider
            && let Err(err) = provider.force_flush()
        {
            failures.push(("metrics", err));
        }
        if let Some(provider) = &self.logger_provider
            && let Err(err) = provider.force_flush()
        {
            failures.push(("logs", err));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(LifecycleError { failures })
        }
    }

    /// Flushes and stops the exporters, reporting the signals that failed.
    /// Dropping the guard does the same, best-effort, with failures printed
    /// to stderr.
    pub fn shutdown(mut self) -> Result<(), LifecycleError> {
        self.shutdown_now()
    }

    /// Shuts each provider down once: later calls find them already taken.
    fn shutdown_now(&mut self) -> Result<(), LifecycleError> {
        let mut failures = Vec::new();
        if let Some(provider) = self.tracer_provider.take()
            && let Err(err) = provider.shutdown()
        {
            failures.push(("traces", err));
        }
        if let Some(provider) = self.meter_provider.take()
            && let Err(err) = provider.shutdown()
        {
            failures.push(("metrics", err));
        }
        if let Some(provider) = self.logger_provider.take()
            && let Err(err) = provider.shutdown()
        {
            failures.push(("logs", err));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(LifecycleError { failures })
        }
    }

    /// Reports a process-fatal `main` result through `tracing`, then flushes
    /// (best-effort, failures printed to stderr).
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
            if let Err(err) = self.force_flush() {
                eprintln!("telemetry: flush failed ({err})");
            }
        }
    }
}

// Best-effort: by now the log pipeline is the thing being torn down.
impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Err(err) = self.shutdown_now() {
            eprintln!("telemetry: shutdown failed ({err})");
        }
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    use super::*;

    fn guard_with(provider: SdkTracerProvider) -> OtelGuard {
        OtelGuard {
            service: "svc".to_string(),
            tracer_provider: Some(provider),
            meter_provider: None,
            logger_provider: None,
            exporter_failures: Vec::new(),
        }
    }

    #[test]
    fn flush_and_shutdown_report_their_outcome() {
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(InMemorySpanExporter::default())
            .build();
        let guard = guard_with(provider);

        guard.force_flush().expect("flushing healthy providers works");
        guard.shutdown().expect("shutting down works");
    }

    #[test]
    fn a_failing_signal_is_named_in_the_error() {
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(InMemorySpanExporter::default())
            .build();
        let guard = guard_with(provider.clone());

        // Shut the provider down behind the guard's back, so the guard's own
        // shutdown fails. (Flushing an already-stopped provider is an SDK
        // no-op, so flush cannot be failed this way.)
        provider.shutdown().expect("first shutdown works");

        let err = guard.shutdown().expect_err("second shutdown fails");
        assert!(err.to_string().contains("traces"), "{err}");
        assert_eq!(err.failures().len(), 1);
        assert_eq!(err.failures()[0].0, "traces");
    }
}
