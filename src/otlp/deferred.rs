// SPDX-License-Identifier: MIT

//! OTLP export whose configuration arrives after the subscriber is installed.
//!
//! A `tracing` subscriber is installed once per process, and a layer added to
//! it later would miss the per-layer filter registration and the downcasting
//! that `tracing-opentelemetry` relies on. So the OTLP trace and log layers are
//! installed at init, filtered `off`, over providers whose only processor is a
//! slot. Attaching fills the slot with a real batch processor once, sets the
//! filters and the rate limit from the configuration, and builds the metric
//! pipeline, which is not a `tracing` layer and can be installed at any time.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use opentelemetry::Context;
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{BatchLogProcessor, LogProcessor, SdkLogRecord, SdkLoggerProvider};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{
    BatchSpanProcessor, SdkTracerProvider, Span, SpanData, SpanProcessor,
};
use tracing_subscriber::filter::EnvFilter;

use super::config::OtlpConfig;
use super::providers::{
    build_log_exporter, build_meter_provider, build_trace_exporter, effective_service_name,
    resource,
};
use super::rate_limit::RateLimitFilter;
use crate::error::TelemetryError;
use crate::reload::{OtlpFilter, ReloadCallback, SharedOtlpFilter};

/// The filter of the deferred layers until a configuration is attached.
pub(crate) const DEFERRED_FILTER: &str = "off";

/// A span processor that forwards to a batch processor once one is attached
/// and drops nothing before, because its layer is filtered `off` until then.
#[derive(Clone, Debug, Default)]
struct DeferredSpanProcessor {
    inner: Arc<OnceLock<BatchSpanProcessor>>,
}

impl SpanProcessor for DeferredSpanProcessor {
    fn on_start(&self, span: &mut Span, cx: &Context) {
        if let Some(inner) = self.inner.get() {
            inner.on_start(span, cx);
        }
    }

    fn on_end(&self, span: SpanData) {
        if let Some(inner) = self.inner.get() {
            inner.on_end(span);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.get().map_or(Ok(()), SpanProcessor::force_flush)
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner
            .get()
            .map_or(Ok(()), |inner| inner.shutdown_with_timeout(timeout))
    }
}

/// The log counterpart of [`DeferredSpanProcessor`].
#[derive(Clone, Debug, Default)]
struct DeferredLogProcessor {
    inner: Arc<OnceLock<BatchLogProcessor>>,
}

impl LogProcessor for DeferredLogProcessor {
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        if let Some(inner) = self.inner.get() {
            inner.emit(data, instrumentation);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.get().map_or(Ok(()), LogProcessor::force_flush)
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner
            .get()
            .map_or(Ok(()), |inner| inner.shutdown_with_timeout(timeout))
    }
}

/// The providers the deferred layers are built over, and their slots.
pub(crate) struct DeferredProviders {
    /// Provider of the deferred trace layer.
    pub tracer_provider: SdkTracerProvider,
    /// Provider of the deferred log layer.
    pub logger_provider: SdkLoggerProvider,
    spans: DeferredSpanProcessor,
    logs: DeferredLogProcessor,
}

/// Builds providers with empty processor slots.
///
/// # Returns
///
/// Providers that export nothing until [`DeferredOtlp::attach`].
pub(crate) fn build_deferred_providers() -> DeferredProviders {
    let spans = DeferredSpanProcessor::default();
    let logs = DeferredLogProcessor::default();
    DeferredProviders {
        tracer_provider: SdkTracerProvider::builder()
            .with_span_processor(spans.clone())
            .build(),
        logger_provider: SdkLoggerProvider::builder()
            .with_log_processor(logs.clone())
            .build(),
        spans,
        logs,
    }
}

/// The installed, not yet configured OTLP export.
pub(crate) struct DeferredOtlp {
    service_name: String,
    spans: DeferredSpanProcessor,
    logs: DeferredLogProcessor,
    filter: ReloadCallback,
    rate_limit: RateLimitFilter,
}

impl DeferredOtlp {
    /// Bundles the slots of `providers` with the controls of their layers.
    ///
    /// # Arguments
    ///
    /// * `service_name` - The builder's service name, used unless the attached
    ///   configuration overrides it.
    /// * `providers` - The providers the deferred layers were built over.
    /// * `filter` - Reloads the filters of both deferred layers.
    /// * `rate_limit` - The rate limit installed on the deferred log layer.
    ///
    /// # Returns
    ///
    /// The handle [`DeferredOtlp::attach`] consumes.
    pub(crate) fn new(
        service_name: String,
        providers: &DeferredProviders,
        filter: ReloadCallback,
        rate_limit: RateLimitFilter,
    ) -> Self {
        Self {
            service_name,
            spans: providers.spans.clone(),
            logs: providers.logs.clone(),
            filter,
            rate_limit,
        }
    }

    /// Starts exporting with `config`.
    ///
    /// Everything fallible — the filter expression and the three exporters —
    /// is built before anything is installed, so a failed attach leaves the
    /// layers filtered `off`.
    ///
    /// # Arguments
    ///
    /// * `config` - Collector endpoint, headers, filter, resource, and rate
    ///   limit.
    /// * `control` - The shared OTLP filter domain, filled on success so log
    ///   control can manage the filter.
    ///
    /// # Returns
    ///
    /// The metric pipeline, installed as the global meter provider, for the
    /// guard to shut down.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError`] when the filter does not parse, an exporter
    /// cannot be built, or the filters cannot be reloaded.
    pub(crate) fn attach(
        self,
        config: &OtlpConfig,
        control: &SharedOtlpFilter,
    ) -> Result<SdkMeterProvider, TelemetryError> {
        EnvFilter::try_new(config.log_level.as_str()).map_err(TelemetryError::subscriber)?;
        let resource = resource(&effective_service_name(&self.service_name, config));
        let mut spans = BatchSpanProcessor::builder(build_trace_exporter(config)?).build();
        spans.set_resource(&resource);
        let mut logs = BatchLogProcessor::builder(build_log_exporter(config)?).build();
        logs.set_resource(&resource);
        let meter_provider = build_meter_provider(config, resource)?;

        // The slots are filled once: this handle is consumed by the attach.
        if self.spans.inner.set(spans).is_err() || self.logs.inner.set(logs).is_err() {
            return Err(TelemetryError::LateConfigurationUnavailable);
        }
        self.rate_limit.set_limit(config.log_rate_limit_per_sec);
        // Reload and publish under the lock log control takes, so an update
        // through log control cannot interleave with this one.
        let mut control = control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (self.filter)(config.log_level.clone()).map_err(TelemetryError::FilterReload)?;
        *control = Some(OtlpFilter {
            current: config.log_level.clone(),
            reload: self.filter,
        });
        Ok(meter_provider)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use opentelemetry_sdk::trace::SpanProcessor;

    use super::{DeferredSpanProcessor, build_deferred_providers};

    #[test]
    fn an_empty_slot_flushes_and_shuts_down_cleanly() {
        let processor = DeferredSpanProcessor::default();
        assert!(processor.force_flush().is_ok());
        assert!(
            processor
                .shutdown_with_timeout(Duration::from_millis(10))
                .is_ok()
        );

        let providers = build_deferred_providers();
        assert!(providers.tracer_provider.shutdown().is_ok());
        assert!(providers.logger_provider.shutdown().is_ok());
    }
}
