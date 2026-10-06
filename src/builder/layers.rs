// SPDX-License-Identifier: MIT

//! Construction helpers for the individual subscriber layers used by
//! [`TelemetryBuilder::init`](crate::TelemetryBuilder::init).
//!
//! Every helper returns its layer with concrete `impl Layer<S>` types so the
//! subscriber can be composed via static `.with(...)` calls in `init()`. Every
//! filter is wrapped in a [`tracing_subscriber::reload::Layer`] and returned
//! with a [`ReloadCallback`], so runtime log control and late configuration can
//! replace it after the subscriber is installed.

use tracing_subscriber::Layer;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::reload;

#[cfg(any(feature = "journald", feature = "otlp"))]
use crate::error::TelemetryError;
use crate::reload::{ReloadCallback, reload_callback};

/// Builds the stdout formatting layer and its filter reload callback.
///
/// # Arguments
///
/// * `stdout_filter` - Initial filter for formatted stdout events.
///
/// # Returns
///
/// A subscriber layer composable onto any subscriber `S` and the callback that
/// replaces its filter at runtime.
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
    (layer, reload_callback(filter_handle))
}

/// Builds the journald layer and its filter reload callback.
///
/// # Arguments
///
/// * `filter_spec` - Initial filter expression shared with local logging.
///
/// # Returns
///
/// A journald layer composable onto any subscriber `S` and the callback that
/// replaces its filter at runtime.
///
/// # Errors
///
/// Returns [`TelemetryError`] when the filter cannot be parsed or the journald
/// layer cannot be constructed.
#[cfg(feature = "journald")]
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
    Ok((layer, reload_callback(filter_handle)))
}

/// Builds the OTLP trace export layer for subscriber `S`.
///
/// # Arguments
///
/// * `tracer_provider` - Provider whose tracer receives exported spans.
/// * `filter_spec` - Initial filter expression of the layer.
///
/// # Returns
///
/// The layer and the callback that replaces its filter at runtime.
///
/// # Errors
///
/// Returns [`TelemetryError`] when the filter cannot be parsed.
#[cfg(feature = "otlp")]
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
    Ok((layer, reload_callback(filter_handle)))
}

/// Builds the OTLP log export layer for subscriber `S`.
///
/// # Arguments
///
/// * `logger_provider` - Provider whose logger receives exported events.
/// * `filter_spec` - Initial filter expression of the layer.
/// * `rate_limit` - Rate limit applied after the filter.
///
/// # Returns
///
/// The layer and the callback that replaces its filter at runtime.
///
/// # Errors
///
/// Returns [`TelemetryError`] when the filter cannot be parsed.
#[cfg(feature = "otlp")]
pub(crate) fn build_otlp_log_layer<S>(
    logger_provider: &opentelemetry_sdk::logs::SdkLoggerProvider,
    filter_spec: &str,
    rate_limit: crate::otlp::RateLimitFilter,
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
            .with_filter(filter_layer.and(rate_limit));
    Ok((layer, reload_callback(filter_handle)))
}
