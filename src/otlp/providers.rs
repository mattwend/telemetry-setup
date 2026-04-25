// SPDX-License-Identifier: MIT

use opentelemetry::global;
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::SdkTracerProvider;

use super::config::OtlpConfig;
use super::endpoint::{signal_endpoint, signal_headers};
use crate::error::TelemetryError;

/// The fully constructed OTLP providers returned by [`build_providers`].
pub(crate) struct BuiltProviders {
    /// Trace provider used by the tracing OpenTelemetry layer.
    pub tracer_provider: SdkTracerProvider,
    /// Log provider used by the tracing-to-OpenTelemetry bridge layer.
    pub logger_provider: SdkLoggerProvider,
    /// Meter provider installed into the OpenTelemetry global meter registry.
    pub meter_provider: SdkMeterProvider,
    /// The final `service.name` value applied to all emitted resources.
    #[cfg(test)]
    pub effective_service_name: String,
}

/// Builds OTLP trace, log, and metric providers.
///
/// The constructed meter provider is also installed as the OpenTelemetry global
/// meter provider so background collectors can emit OTLP-backed metrics.
///
/// # Arguments
///
/// * `service_name` - Default service name used when `config` does not provide
///   an override.
/// * `config` - OTLP collector endpoint, resource, filter, header, and
///   rate-limit settings.
///
/// # Returns
///
/// The constructed providers and the effective service name applied to their
/// resources.
///
/// # Errors
///
/// Returns [`TelemetryError`] when an OTLP signal endpoint is invalid or any
/// trace, log, or metric provider cannot be constructed.
pub(crate) fn build_providers(
    service_name: &str,
    config: &OtlpConfig,
) -> Result<BuiltProviders, TelemetryError> {
    let effective_service_name = config
        .service_name
        .clone()
        .unwrap_or_else(|| service_name.to_string());
    let resource = Resource::builder()
        .with_service_name(effective_service_name.clone())
        .build();

    let trace_exporter = build_trace_exporter(config)?;
    let log_exporter = build_log_exporter(config)?;
    let metric_exporter = build_metric_exporter(config)?;

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(trace_exporter)
        .build();

    let logger_provider = SdkLoggerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(log_exporter)
        .build();

    let reader = PeriodicReader::builder(metric_exporter)
        .with_interval(config.metrics_interval)
        .build();
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_reader(reader)
        .build();
    global::set_meter_provider(meter_provider.clone());

    Ok(BuiltProviders {
        tracer_provider,
        logger_provider,
        meter_provider,
        #[cfg(test)]
        effective_service_name,
    })
}

/// Builds the OTLP trace exporter from `config`.
fn build_trace_exporter(
    config: &OtlpConfig,
) -> Result<opentelemetry_otlp::SpanExporter, TelemetryError> {
    opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(signal_endpoint(&config.url, "v1/traces", "traces")?)
        .with_headers(signal_headers(config, &config.headers.traces))
        .build()
        .map_err(TelemetryError::trace_provider)
}

/// Builds the OTLP log exporter from `config`.
fn build_log_exporter(
    config: &OtlpConfig,
) -> Result<opentelemetry_otlp::LogExporter, TelemetryError> {
    opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(signal_endpoint(&config.url, "v1/logs", "logs")?)
        .with_headers(signal_headers(config, &config.headers.logs))
        .build()
        .map_err(TelemetryError::log_provider)
}

/// Builds the OTLP metric exporter from `config`.
fn build_metric_exporter(
    config: &OtlpConfig,
) -> Result<opentelemetry_otlp::MetricExporter, TelemetryError> {
    opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(signal_endpoint(&config.url, "v1/metrics", "metrics")?)
        .with_headers(signal_headers(config, &config.headers.metrics))
        .build()
        .map_err(TelemetryError::meter_provider)
}

#[cfg(test)]
mod tests {
    use super::build_providers;
    use crate::error::TelemetryError;
    use crate::otlp::OtlpConfig;

    #[test]
    fn providers_use_service_name_override_when_present() {
        let providers = build_providers(
            "controller",
            &OtlpConfig {
                url: "http://localhost:4318".to_string(),
                service_name: Some("custom-controller".to_string()),
                ..OtlpConfig::default()
            },
        )
        .unwrap();

        assert_eq!(providers.effective_service_name, "custom-controller");
        let _ = providers.logger_provider.shutdown();
        let _ = providers.meter_provider.shutdown();
        let _ = providers.tracer_provider.shutdown();
    }

    #[test]
    fn providers_fall_back_to_builder_service_name() {
        let providers = build_providers(
            "controller",
            &OtlpConfig {
                url: "http://localhost:4318".to_string(),
                service_name: None,
                ..OtlpConfig::default()
            },
        )
        .unwrap();

        assert_eq!(providers.effective_service_name, "controller");
        let _ = providers.logger_provider.shutdown();
        let _ = providers.meter_provider.shutdown();
        let _ = providers.tracer_provider.shutdown();
    }

    #[test]
    fn providers_report_invalid_otlp_endpoint() {
        let result = build_providers(
            "controller",
            &OtlpConfig {
                url: "://bad".to_string(),
                ..OtlpConfig::default()
            },
        );

        assert!(matches!(
            result,
            Err(TelemetryError::OtlpEndpoint {
                signal: "traces",
                ..
            })
        ));
    }
}
