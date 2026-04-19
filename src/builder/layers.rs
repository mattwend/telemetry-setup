// SPDX-License-Identifier: MIT

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::reload;
use tracing_subscriber::{Layer, Registry};

#[cfg(any(feature = "journald", feature = "otlp"))]
use crate::error::TelemetryError;

pub(crate) type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;
pub(crate) type FilterReloadHandle = reload::Handle<EnvFilter, Registry>;

/// Builds the stdout formatting layer and its reload handle.
///
/// # Arguments
///
/// * `stdout_filter` - Initial filter for formatted stdout events.
///
/// # Returns
///
/// A boxed subscriber layer and the reload handle used to update its filter.
pub(crate) fn build_fmt_layer(stdout_filter: EnvFilter) -> (BoxedLayer, FilterReloadHandle) {
    let (filter_layer, filter_handle) = reload::Layer::new(stdout_filter);
    let layer = tracing_subscriber::fmt::layer().with_filter(filter_layer);
    (Box::new(layer), filter_handle)
}

/// Builds the journald layer and its reload handle.
///
/// # Arguments
///
/// * `filter_spec` - Initial filter expression shared with local logging.
///
/// # Returns
///
/// A boxed journald layer and the reload handle used to update its filter.
///
/// # Errors
///
/// Returns [`TelemetryError`] when the filter cannot be parsed or the journald
/// layer cannot be constructed.
#[cfg(feature = "journald")]
pub(crate) fn build_journald_layer(
    filter_spec: &str,
) -> Result<(BoxedLayer, FilterReloadHandle), TelemetryError> {
    let journald_filter = EnvFilter::try_new(filter_spec).map_err(TelemetryError::subscriber)?;
    let (filter_layer, filter_handle) = reload::Layer::new(journald_filter);
    let layer = tracing_journald::layer()
        .map_err(TelemetryError::subscriber)?
        .with_filter(filter_layer);
    Ok((Box::new(layer), filter_handle))
}

/// Built OTLP layers, reload handles, and providers that must be kept alive.
#[cfg(feature = "otlp")]
pub(crate) struct OtlpLayerParts {
    /// Trace export layer.
    pub trace_layer: BoxedLayer,
    /// Log export layer.
    pub log_layer: BoxedLayer,
    /// Reload handle for the trace layer filter.
    pub trace_reload_handle: FilterReloadHandle,
    /// Reload handle for the log layer filter.
    pub log_reload_handle: FilterReloadHandle,
    /// Providers backing the OTLP layers.
    pub providers: crate::otlp::BuiltProviders,
}

/// Builds OTLP trace and log layers, their reload handles, and backing providers.
///
/// # Arguments
///
/// * `service_name` - Default service name for OTLP resource attributes.
/// * `otlp_config` - OTLP exporter configuration.
///
/// # Returns
///
/// Constructed OTLP layers plus provider guard parts.
///
/// # Errors
///
/// Returns [`TelemetryError`] when providers cannot be built or filters cannot be parsed.
#[cfg(feature = "otlp")]
pub(crate) fn build_otlp_layers(
    service_name: &str,
    otlp_config: &crate::otlp::OtlpConfig,
) -> Result<OtlpLayerParts, TelemetryError> {
    use opentelemetry::trace::TracerProvider;
    use tracing_subscriber::filter::FilterExt;

    let providers = crate::otlp::build_providers(service_name, otlp_config)?;
    let tracer = providers
        .tracer_provider
        .tracer(providers.effective_service_name.clone());

    let otlp_trace_filter =
        EnvFilter::try_new(otlp_config.log_level.as_str()).map_err(TelemetryError::subscriber)?;
    let otlp_log_filter =
        EnvFilter::try_new(otlp_config.log_level.as_str()).map_err(TelemetryError::subscriber)?;
    let (trace_filter_layer, trace_filter_handle) = reload::Layer::new(otlp_trace_filter);
    let (log_filter_layer, log_filter_handle) = reload::Layer::new(otlp_log_filter);

    let trace_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_filter(trace_filter_layer);

    let log_layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
        &providers.logger_provider,
    )
    .with_filter(
        log_filter_layer.and(crate::otlp::RateLimitFilter::new_optional(
            otlp_config.log_rate_limit_per_sec,
        )),
    );

    Ok(OtlpLayerParts {
        trace_layer: Box::new(trace_layer),
        log_layer: Box::new(log_layer),
        trace_reload_handle: trace_filter_handle,
        log_reload_handle: log_filter_handle,
        providers,
    })
}
