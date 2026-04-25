// SPDX-License-Identifier: MIT

#[cfg(all(
    feature = "otlp",
    feature = "log-control",
    feature = "journald",
    feature = "tokio-metrics"
))]
#[test]
fn readme_common_configuration_compiles() {
    let _ = compile_readme_common_configuration;
}

#[cfg(all(
    feature = "otlp",
    feature = "log-control",
    feature = "journald",
    feature = "tokio-metrics"
))]
fn compile_readme_common_configuration() -> Result<(), telemetry_setup::TelemetryError> {
    use std::collections::HashMap;

    use telemetry_setup::{LogControlConfig, OtlpConfig, OtlpHeadersConfig, TelemetryBuilder};

    let _telemetry = TelemetryBuilder::new("controller")
        .with_stdout_filter("info")
        .with_otlp_config(OtlpConfig {
            url: "http://localhost:4318".to_string(),
            headers: OtlpHeadersConfig {
                common: HashMap::from([("X-Greptime-DB-Name".to_string(), "edge".to_string())]),
                traces: HashMap::from([(
                    "x-greptime-pipeline-name".to_string(),
                    "greptime_trace_v1".to_string(),
                )]),
                logs: HashMap::from([(
                    "x-greptime-pipeline-name".to_string(),
                    "greptime_identity".to_string(),
                )]),
                ..OtlpHeadersConfig::default()
            },
            ..OtlpConfig::default()
        })
        .with_log_control(LogControlConfig { port: 6669 })
        .enable_journald()
        .enable_tokio_metrics()
        .init()?;

    Ok(())
}
