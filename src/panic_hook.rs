//! Panic-to-`tracing` bridge.

use std::sync::Once;

/// Routes panics through `tracing` so they reach the telemetry backend.
///
/// Panics bypass `tracing` entirely: the runtime hands them to a global panic
/// hook that writes to stderr, so nothing about them reaches the OTLP log
/// bridge. That blind spot is widest for a panic inside a spawned task, which
/// kills only that task. The process keeps running with no record anywhere
/// that the task died.
///
/// This replaces the hook with one that emits an ERROR event first and then
/// chains to the previous hook, so the stderr output is unchanged. Installed
/// during init unless the config opts out. Idempotent, so repeated calls
/// never stack hooks.
///
/// The backtrace is captured only when `RUST_BACKTRACE` is set. `capture()`
/// is a no-op otherwise, which keeps the hook cheap on the default path.
pub(crate) fn install_panic_hook() {
    static INSTALLED: Once = Once::new();

    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();

        std::panic::set_hook(Box::new(move |info| {
            let payload = info.payload();
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");

            let location = info
                .location()
                .map(ToString::to_string)
                .unwrap_or_else(|| "unknown".to_string());

            let thread = std::thread::current();
            let thread = thread.name().unwrap_or("unnamed").to_string();

            let backtrace = std::backtrace::Backtrace::capture();
            let backtrace = match backtrace.status() {
                std::backtrace::BacktraceStatus::Captured => backtrace.to_string(),
                _ => "<disabled, set RUST_BACKTRACE=1>".to_string(),
            };

            tracing::error!(
                panic.message = %message,
                panic.location = %location,
                panic.thread = %thread,
                panic.backtrace = %backtrace,
                "thread panicked"
            );

            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
    use tracing_subscriber::registry::LookupSpan;

    use super::install_panic_hook;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<String>>>);

    impl Visit for Captured {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .expect("captured events lock")
                .push(format!("{}={value:?}", field.name()));
        }
    }

    impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Captured {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            event.record(&mut self.clone());
        }
    }

    #[test]
    fn panics_are_emitted_as_tracing_events() {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());

        tracing::subscriber::with_default(subscriber, || {
            install_panic_hook();
            let _ = std::panic::catch_unwind(|| panic!("boom"));
        });

        let events = captured.0.lock().expect("captured events lock").join("\n");
        assert!(events.contains("thread panicked"), "{events}");
        assert!(events.contains("boom"), "{events}");
        assert!(events.contains("panic_hook.rs"), "{events}");
    }
}
