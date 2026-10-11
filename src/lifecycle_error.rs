//! The error from flushing or shutting telemetry down.

use std::fmt;

use opentelemetry_sdk::error::OTelSdkError;

/// One or more signals failed to flush or shut down.
#[derive(Debug)]
pub struct LifecycleError {
    pub(crate) failures: Vec<(&'static str, OTelSdkError)>,
}

impl LifecycleError {
    /// What failed, per signal: `traces`, `metrics` or `logs`.
    pub fn failures(&self) -> &[(&'static str, OTelSdkError)] {
        &self.failures
    }
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (signal, err)) in self.failures.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{signal}: {err}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.failures
            .first()
            .map(|(_, err)| err as &(dyn std::error::Error + 'static))
    }
}
