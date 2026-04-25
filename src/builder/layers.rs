// SPDX-License-Identifier: MIT

//! Construction helpers for the individual subscriber layers used by
//! [`TelemetryBuilder::init`](crate::TelemetryBuilder::init).
//!
//! Every helper returns its layer with concrete `impl Layer<S>` types so the
//! subscriber can be composed via static `.with(...)` calls in `init()`. When
//! runtime log control is enabled, helpers that expose mutable filters wrap
//! those filters in [`tracing_subscriber::reload::Layer`] so the active filter
//! can be updated at runtime.

use tracing_subscriber::Layer;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::reload;

#[cfg(any(feature = "journald", feature = "otlp", feature = "log-control"))]
use crate::error::TelemetryError;

#[cfg(feature = "log-control")]
use crate::log_control::ReloadCallback;

/// Builds the stdout formatting layer and its filter reload handle.
///
/// # Arguments
///
/// * `stdout_filter` - Initial filter for formatted stdout events.
///
/// # Returns
///
/// A subscriber layer composable onto any subscriber `S` and the reload handle
/// used to update its filter at runtime.
#[cfg(feature = "log-control")]
pub(crate) fn build_fmt_layer<S>(
    stdout_filter: EnvFilter,
) -> (impl Layer<S> + Send + Sync + 'static, ReloadCallback)
where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
        + Send
        + Sync
        + 'static,
{
    let (filter_layer, filter_handle) = reload::Layer::new(stdout_filter);
    let layer = tracing_subscriber::fmt::layer().with_filter(filter_layer);
    let reload = build_reload_callback(filter_handle);
    (layer, reload)
}

#[cfg(not(feature = "log-control"))]
pub(crate) fn build_fmt_layer<S>(
    stdout_filter: EnvFilter,
) -> (impl Layer<S> + Send + Sync + 'static, ())
where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
        + Send
        + Sync
        + 'static,
{
    let (filter_layer, _) = reload::Layer::new(stdout_filter);
    let layer = tracing_subscriber::fmt::layer().with_filter(filter_layer);
    (layer, ())
}

/// Builds the journald layer and its filter reload handle.
///
/// # Arguments
///
/// * `filter_spec` - Initial filter expression shared with local logging.
///
/// # Returns
///
/// A journald layer composable onto any subscriber `S` and the reload handle
/// used to update its filter at runtime.
///
/// # Errors
///
/// Returns [`TelemetryError`] when the filter cannot be parsed or the journald
/// layer cannot be constructed.
#[cfg(all(feature = "journald", feature = "log-control"))]
pub(crate) fn build_journald_layer<S>(
    filter_spec: &str,
) -> Result<(impl Layer<S> + Send + Sync + 'static, ReloadCallback), TelemetryError>
where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
        + Send
        + Sync
        + 'static,
{
    let journald_filter = EnvFilter::try_new(filter_spec).map_err(TelemetryError::subscriber)?;
    let (filter_layer, filter_handle) = reload::Layer::new(journald_filter);
    let layer = tracing_journald::layer()
        .map_err(TelemetryError::subscriber)?
        .with_filter(filter_layer);
    let reload = build_reload_callback(filter_handle);
    Ok((layer, reload))
}

#[cfg(all(feature = "journald", not(feature = "log-control")))]
pub(crate) fn build_journald_layer<S>(
    filter_spec: &str,
) -> Result<(impl Layer<S> + Send + Sync + 'static, ()), TelemetryError>
where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
        + Send
        + Sync
        + 'static,
{
    let journald_filter = EnvFilter::try_new(filter_spec).map_err(TelemetryError::subscriber)?;
    let (filter_layer, _) = reload::Layer::new(journald_filter);
    let layer = tracing_journald::layer()
        .map_err(TelemetryError::subscriber)?
        .with_filter(filter_layer);
    Ok((layer, ()))
}

/// Built OTLP providers.
#[cfg(feature = "otlp")]
pub(crate) struct OtlpLayerParts {
    /// Providers backing the OTLP layers; kept alive by the guard.
    pub providers: crate::otlp::BuiltProviders,
}

/// Builds OTLP providers.
///
/// # Arguments
///
/// * `service_name` - Default service name for OTLP resource attributes.
/// * `otlp_config` - OTLP exporter configuration.
///
/// # Returns
///
/// Constructed OTLP providers.
///
/// # Errors
///
/// Returns [`TelemetryError`] when providers cannot be built.
#[cfg(feature = "otlp")]
pub(crate) fn build_otlp_parts(
    service_name: &str,
    otlp_config: &crate::otlp::OtlpConfig,
) -> Result<OtlpLayerParts, TelemetryError> {
    let providers = crate::otlp::build_providers(service_name, otlp_config)?;

    Ok(OtlpLayerParts { providers })
}

/// Builds the OTLP trace export layer for subscriber `S`.
#[cfg(all(feature = "otlp", feature = "log-control"))]
pub(crate) fn build_otlp_trace_layer<S>(
    tracer_provider: &opentelemetry_sdk::trace::SdkTracerProvider,
    filter_spec: &str,
) -> Result<(impl Layer<S> + Send + Sync + 'static, ReloadCallback), TelemetryError>
where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
        + Send
        + Sync
        + 'static,
{
    use opentelemetry::trace::TracerProvider;

    let trace_filter = EnvFilter::try_new(filter_spec).map_err(TelemetryError::subscriber)?;
    let (filter_layer, filter_handle) = reload::Layer::new(trace_filter);
    let tracer = tracer_provider.tracer("telemetry-setup");
    let layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_filter(filter_layer);
    let reload = build_reload_callback(filter_handle);
    Ok((layer, reload))
}

/// Builds the OTLP trace export layer for subscriber `S`.
#[cfg(all(feature = "otlp", not(feature = "log-control")))]
pub(crate) fn build_otlp_trace_layer<S>(
    tracer_provider: &opentelemetry_sdk::trace::SdkTracerProvider,
    filter_spec: &str,
) -> Result<impl Layer<S> + Send + Sync + 'static, TelemetryError>
where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
        + Send
        + Sync
        + 'static,
{
    use opentelemetry::trace::TracerProvider;

    let trace_filter = EnvFilter::try_new(filter_spec).map_err(TelemetryError::subscriber)?;
    let tracer = tracer_provider.tracer("telemetry-setup");
    Ok(tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_filter(trace_filter))
}

/// Builds the OTLP log export layer for subscriber `S`.
#[cfg(all(feature = "otlp", feature = "log-control"))]
pub(crate) fn build_otlp_log_layer<S>(
    logger_provider: &opentelemetry_sdk::logs::SdkLoggerProvider,
    filter_spec: &str,
    rate_limit_per_sec: Option<u64>,
) -> Result<(impl Layer<S> + Send + Sync + 'static, ReloadCallback), TelemetryError>
where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
        + Send
        + Sync
        + 'static,
{
    use tracing_subscriber::filter::FilterExt;

    let log_filter = EnvFilter::try_new(filter_spec).map_err(TelemetryError::subscriber)?;
    let (filter_layer, filter_handle) = reload::Layer::new(log_filter);
    let layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(logger_provider)
            .with_filter(filter_layer.and(crate::otlp::RateLimitFilter::new_optional(
                rate_limit_per_sec,
            )));
    let reload = build_reload_callback(filter_handle);
    Ok((layer, reload))
}

/// Builds the OTLP log export layer for subscriber `S`.
#[cfg(all(feature = "otlp", not(feature = "log-control")))]
pub(crate) fn build_otlp_log_layer<S>(
    logger_provider: &opentelemetry_sdk::logs::SdkLoggerProvider,
    filter_spec: &str,
    rate_limit_per_sec: Option<u64>,
) -> Result<impl Layer<S> + Send + Sync + 'static, TelemetryError>
where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
        + Send
        + Sync
        + 'static,
{
    use tracing_subscriber::filter::FilterExt;

    let log_filter = EnvFilter::try_new(filter_spec).map_err(TelemetryError::subscriber)?;
    Ok(
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(logger_provider)
            .with_filter(log_filter.and(crate::otlp::RateLimitFilter::new_optional(
                rate_limit_per_sec,
            ))),
    )
}

#[cfg(feature = "log-control")]
fn build_reload_callback<S>(handle: reload::Handle<EnvFilter, S>) -> ReloadCallback
where
    S: tracing::Subscriber + Send + Sync + 'static,
{
    std::sync::Arc::new(move |spec: String| {
        let filter = EnvFilter::try_new(spec.as_str())
            .map_err(|error| TelemetryError::subscriber(error).to_string())?;
        handle
            .reload(filter)
            .map_err(|error| TelemetryError::subscriber(error).to_string())
    })
}
