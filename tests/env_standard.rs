//! Standard SDK environment variables and explicit code overrides. Each
//! scenario runs in its own process so environment and telemetry globals
//! cannot leak between tests.

#![cfg(feature = "stdout")]

use std::process::Command;
use std::time::Duration;

use telemetry::opentelemetry::{
    global,
    trace::{Span as _, Tracer as _},
};
use telemetry::{Exporter, TelemetryConfig};

#[test]
fn standard_environment_defaults_and_code_overrides() {
    if let Ok(scenario) = std::env::var("TELEMETRY_TEST_ENV_SCENARIO") {
        run_scenario(&scenario);
        return;
    }

    for (scenario, ratio, expected_attributes) in [
        ("standard", "1", Some(["env-version", "env-deployment"])),
        ("standard-new", "1", Some(["env-version", "env-deployment"])),
        ("unsampled", "0", None),
        (
            "override-before",
            "0",
            Some(["code-version", "code-deployment"]),
        ),
        (
            "override-after",
            "0",
            Some(["code-version", "code-deployment"]),
        ),
    ] {
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "standard_environment_defaults_and_code_overrides",
                "--nocapture",
            ])
            .env("TELEMETRY_TEST_ENV_SCENARIO", scenario)
            .env(
                "OTEL_RESOURCE_ATTRIBUTES",
                "service.version=env-version,deployment.environment.name=env-deployment",
            )
            .env("OTEL_TRACES_SAMPLER", "parentbased_traceidratio")
            .env("OTEL_TRACES_SAMPLER_ARG", ratio)
            .env("OTEL_METRIC_EXPORT_INTERVAL", "3600000")
            .env_remove("OTEL_SDK_DISABLED")
            .env_remove("OTEL_SERVICE_NAME")
            .output()
            .expect("run isolated scenario");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{scenario}: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if let Some(attributes) = expected_attributes {
            for value in attributes {
                assert!(
                    stdout.contains(value),
                    "{scenario}: missing {value} in {stdout}"
                );
            }
        }
        if scenario.starts_with("override") {
            assert!(!stdout.contains("env-version"), "{scenario}: {stdout}");
            assert!(!stdout.contains("env-deployment"), "{scenario}: {stdout}");
        }
    }
}

fn run_scenario(scenario: &str) {
    let configure = |config: TelemetryConfig| {
        config
            .service_version("code-version")
            .environment("code-deployment")
            .sample_ratio(1.0)
            .metrics_export_interval(Duration::from_secs(3600))
    };
    let config = match scenario {
        "standard-new" => TelemetryConfig::new("env-test"),
        "override-before" => configure(TelemetryConfig::new("env-test")).with_env(),
        "override-after" => configure(TelemetryConfig::from_env("env-test")),
        _ => TelemetryConfig::from_env("env-test"),
    };
    let guard = config
        .exporter(Exporter::Stdout)
        .log_filter("off")
        .capture_panics(false)
        .try_init()
        .expect("init");
    let span = global::tracer("env-test").start("standard-env");
    assert_eq!(span.is_recording(), scenario != "unsampled", "{scenario}");
    drop(span);
    guard.force_flush().expect("flush");
    guard.shutdown().expect("shutdown");
}
