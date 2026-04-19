// SPDX-License-Identifier: MIT

//! Telemetry setup builder.
//!
//! [`TelemetryBuilder`] owns process-local setup decisions: stdout filtering,
//! optional journald output, optional OTLP export, optional log-control, and
//! optional Tokio runtime metrics. Serializable runtime config such as
//! `OtlpConfig` can be loaded by the consuming service and then passed to the
//! builder when the `otlp` feature is enabled.
//!
//! Builder-owned settings intentionally affect process-local behavior:
//!
//! - [`TelemetryBuilder::with_stdout_filter`] sets the fallback local filter.
//! - [`TelemetryBuilder::with_env_var`] selects the environment variable used
//!   before the fallback filter.
//! - [`TelemetryBuilder::enable_journald`] enables journald output when compiled
//!   with the `journald` feature.
//! - [`TelemetryBuilder::enable_tokio_metrics`] enables Tokio runtime metrics when
//!   compiled with `tokio-metrics` and OTLP is configured.
//! - `with_otlp_config` and `with_log_control` are available behind their
//!   matching crate features.
//!
//! See the repository `examples/` directory for complete binaries.

mod feature_checks;
mod filter;
pub(crate) mod layers;

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::error::TelemetryError;
use crate::guard::TelemetryGuard;
#[cfg(all(feature = "log-control", feature = "otlp"))]
use crate::log_control::otlp_reload_callback;
#[cfg(feature = "log-control")]
use crate::log_control::{
    LogControlConfig, ReloadState, spawn_log_control_server, stdout_reload_callback,
};
#[cfg(feature = "otlp")]
use crate::otlp::OtlpConfig;

use self::filter::EnvLookup;

/// Builder for installing local logging, optional OTLP export, and related helpers.
pub struct TelemetryBuilder {
    #[cfg_attr(not(feature = "otlp"), allow(dead_code))]
    service_name: String,
    #[cfg(feature = "otlp")]
    otlp_config: Option<OtlpConfig>,
    stdout_filter: Option<String>,
    env_var_name: Option<String>,
    env_lookup: EnvLookup,
    #[cfg(feature = "log-control")]
    log_control_config: Option<LogControlConfig>,
    enable_journald: bool,
    enable_tokio_metrics: bool,
}

impl std::fmt::Debug for TelemetryBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("TelemetryBuilder");
        debug.field("service_name", &self.service_name);
        #[cfg(feature = "otlp")]
        debug.field("otlp_config", &self.otlp_config);
        debug.field("stdout_filter", &self.stdout_filter);
        debug.field("env_var_name", &self.env_var_name);
        #[cfg(feature = "log-control")]
        debug.field("log_control_config", &self.log_control_config);
        debug.field("enable_journald", &self.enable_journald);
        debug.field("enable_tokio_metrics", &self.enable_tokio_metrics);
        debug.finish_non_exhaustive()
    }
}

impl TelemetryBuilder {
    /// Creates a builder with local stdout logging enabled.
    ///
    /// # Arguments
    ///
    /// * `service_name` - Default `service.name` resource value used for OTLP export.
    ///
    /// # Returns
    ///
    /// A new builder with stdout logging enabled, `RUST_LOG` as the initial filter
    /// environment variable, and all optional exporters and helpers disabled.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            #[cfg(feature = "otlp")]
            otlp_config: None,
            stdout_filter: None,
            env_var_name: Some("RUST_LOG".to_string()),
            env_lookup: Arc::new(|name| std::env::var(name).ok()),
            #[cfg(feature = "log-control")]
            log_control_config: None,
            enable_journald: false,
            enable_tokio_metrics: false,
        }
    }

    /// Enables OTLP export using the provided configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Collector endpoint, filter, resource, and rate-limit settings.
    ///
    /// # Returns
    ///
    /// The updated builder.
    #[cfg(feature = "otlp")]
    #[cfg_attr(docsrs, doc(cfg(feature = "otlp")))]
    pub fn with_otlp_config(mut self, config: OtlpConfig) -> Self {
        self.otlp_config = Some(config);
        self
    }

    /// Sets an explicit stdout and local logging filter string.
    ///
    /// # Arguments
    ///
    /// * `filter` - `tracing_subscriber::EnvFilter` expression used when no
    ///   configured environment variable is present.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn with_stdout_filter(mut self, filter: impl Into<String>) -> Self {
        self.stdout_filter = Some(filter.into());
        self
    }

    /// Sets the environment variable name used to source the initial local filter.
    ///
    /// # Arguments
    ///
    /// * `name` - Environment variable read before the explicit stdout filter
    ///   fallback.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn with_env_var(mut self, name: impl Into<String>) -> Self {
        self.env_var_name = Some(name.into());
        self
    }

    /// Disables reading the local filter from the process environment.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn without_env_var(mut self) -> Self {
        self.env_var_name = None;
        self
    }

    /// Sets the environment lookup used to source the initial local filter in tests.
    ///
    /// # Arguments
    ///
    /// * `lookup` - Function that returns the environment value for a variable name.
    ///
    /// # Returns
    ///
    /// The updated builder.
    #[cfg(test)]
    fn with_env_lookup(
        mut self,
        lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.env_lookup = Arc::new(lookup);
        self
    }

    /// Enables the localhost log-control server using the provided configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Port and bind settings for runtime filter updates.
    ///
    /// # Returns
    ///
    /// The updated builder.
    #[cfg(feature = "log-control")]
    #[cfg_attr(docsrs, doc(cfg(feature = "log-control")))]
    pub fn with_log_control(mut self, config: LogControlConfig) -> Self {
        self.log_control_config = Some(config);
        self
    }

    /// Enables journald output when the crate is built with that feature.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn enable_journald(mut self) -> Self {
        self.enable_journald = true;
        self
    }

    /// Disables journald output.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn disable_journald(mut self) -> Self {
        self.enable_journald = false;
        self
    }

    /// Enables or disables journald output when the crate is built with that feature.
    ///
    /// Prefer [`TelemetryBuilder::enable_journald`] or
    /// [`TelemetryBuilder::disable_journald`] in new code.
    ///
    /// # Arguments
    ///
    /// * `enable` - `true` to add a journald layer, or `false` to leave it disabled.
    ///
    /// # Returns
    ///
    /// The updated builder.
    #[deprecated(
        since = "0.1.1",
        note = "use enable_journald() or disable_journald() instead"
    )]
    pub fn with_journald(mut self, enable: bool) -> Self {
        self.enable_journald = enable;
        self
    }

    /// Enables Tokio runtime metric collection.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn enable_tokio_metrics(mut self) -> Self {
        self.enable_tokio_metrics = true;
        self
    }

    /// Disables Tokio runtime metric collection.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn disable_tokio_metrics(mut self) -> Self {
        self.enable_tokio_metrics = false;
        self
    }

    /// Enables or disables Tokio runtime metric collection.
    ///
    /// Prefer [`TelemetryBuilder::enable_tokio_metrics`] or
    /// [`TelemetryBuilder::disable_tokio_metrics`] in new code.
    ///
    /// # Arguments
    ///
    /// * `enable` - `true` to spawn Tokio runtime metric collection, or `false`
    ///   to leave it disabled.
    ///
    /// # Returns
    ///
    /// The updated builder.
    #[deprecated(
        since = "0.1.1",
        note = "use enable_tokio_metrics() or disable_tokio_metrics() instead"
    )]
    pub fn with_tokio_metrics(mut self, enable: bool) -> Self {
        self.enable_tokio_metrics = enable;
        self
    }

    /// Builds and installs the configured tracing subscriber and background tasks.
    ///
    /// # Returns
    ///
    /// A [`TelemetryGuard`] that keeps telemetry providers and background tasks
    /// alive until shutdown.
    ///
    /// This initialization is process-global; calling `init()` more than once in
    /// the same process is unsupported because the tracing subscriber can only be
    /// installed once.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError`] when requested feature-gated functionality is
    /// not compiled in, a filter cannot be parsed, a provider cannot be built,
    /// the subscriber cannot be installed, or the log-control server cannot be
    /// started.
    pub fn init(self) -> Result<TelemetryGuard, TelemetryError> {
        feature_checks::reject_journald_without_feature(self.enable_journald)?;
        feature_checks::reject_tokio_metrics_without_feature(self.enable_tokio_metrics)?;

        #[cfg(all(feature = "tokio-metrics", feature = "otlp"))]
        feature_checks::reject_tokio_metrics_without_otlp_config(
            self.enable_tokio_metrics,
            self.otlp_config.is_some(),
        )?;
        #[cfg(all(feature = "tokio-metrics", not(feature = "otlp")))]
        feature_checks::reject_tokio_metrics_without_otlp_config(self.enable_tokio_metrics, false)?;

        let stdout_spec = self.resolve_stdout_filter();
        let stdout_filter =
            EnvFilter::try_new(stdout_spec.as_str()).map_err(TelemetryError::subscriber)?;
        let (fmt_layer, fmt_filter_handle) = layers::build_fmt_layer(stdout_filter);
        #[cfg(not(feature = "log-control"))]
        let _ = &fmt_filter_handle;
        #[allow(unused_mut)]
        let mut subscriber_layers = vec![fmt_layer];

        #[cfg(feature = "journald")]
        let journald_reload_handle = if self.enable_journald {
            let (journald_layer, journald_reload_handle) =
                layers::build_journald_layer(stdout_spec.as_str())?;
            subscriber_layers.push(journald_layer);
            Some(journald_reload_handle)
        } else {
            None
        };
        #[cfg(all(feature = "journald", not(feature = "log-control")))]
        let _ = &journald_reload_handle;

        #[cfg(all(feature = "log-control", feature = "otlp"))]
        let otlp_current = self
            .otlp_config
            .as_ref()
            .map(|config| config.log_level.clone());
        #[cfg(all(feature = "log-control", not(feature = "otlp")))]
        let otlp_current = None;

        #[cfg(feature = "otlp")]
        let mut otlp_guard_parts = None;
        #[cfg(feature = "otlp")]
        let mut otlp_reload_handles = None;
        #[cfg(feature = "otlp")]
        if let Some(otlp_config) = self.otlp_config.as_ref() {
            let otlp_layers = layers::build_otlp_layers(&self.service_name, otlp_config)?;
            let layers::OtlpLayerParts {
                trace_layer,
                log_layer,
                trace_reload_handle,
                log_reload_handle,
                providers,
            } = otlp_layers;
            subscriber_layers.push(trace_layer);
            subscriber_layers.push(log_layer);
            otlp_reload_handles = Some((trace_reload_handle, log_reload_handle));
            otlp_guard_parts = Some(providers);
        }
        #[cfg(all(feature = "otlp", not(feature = "log-control")))]
        let _ = &otlp_reload_handles;

        tracing_subscriber::registry()
            .with(subscriber_layers)
            .try_init()
            .map_err(TelemetryError::subscriber)?;

        let cancel_token = CancellationToken::new();
        #[allow(unused_mut)]
        let mut guard = TelemetryGuard::new(cancel_token);

        #[cfg(feature = "otlp")]
        if let Some(providers) = otlp_guard_parts {
            guard.tracer_provider = Some(providers.tracer_provider);
            guard.logger_provider = Some(providers.logger_provider);
            guard.meter_provider = Some(providers.meter_provider);
        }

        #[cfg(feature = "tokio-metrics")]
        if self.enable_tokio_metrics {
            let interval = self
                .otlp_config
                .as_ref()
                .map(|config| config.metrics_interval)
                .unwrap_or_else(|| std::time::Duration::from_secs(5));
            let task = crate::tokio_metrics::start_tokio_metrics_monitoring(
                guard.cancel_token.child_token(),
                interval,
            );
            guard.background_tasks.push(task);
        }

        #[cfg(feature = "log-control")]
        if let Some(config) = self.log_control_config {
            #[cfg(feature = "journald")]
            let journald_reload_handle = journald_reload_handle.clone();
            #[cfg(not(feature = "journald"))]
            let journald_reload_handle = None;

            let stdout_reload =
                stdout_reload_callback(fmt_filter_handle.clone(), journald_reload_handle);

            #[cfg(feature = "otlp")]
            let otlp_reload = match otlp_reload_handles {
                Some((trace_handle, log_handle)) => {
                    otlp_reload_callback(Some(trace_handle), Some(log_handle))
                }
                None => None,
            };
            #[cfg(not(feature = "otlp"))]
            let otlp_reload = None;

            let state = ReloadState::new(stdout_spec, otlp_current, stdout_reload, otlp_reload);
            let task = spawn_log_control_server(config, state, guard.cancel_token.child_token())?;
            guard.background_tasks.push(task);
        }

        Ok(guard)
    }

    /// Resolves the local filter from the configured environment variable or fallback string.
    fn resolve_stdout_filter(&self) -> String {
        filter::resolve_stdout_filter(&self.env_var_name, &self.env_lookup, &self.stdout_filter)
    }
}

#[cfg(test)]
mod tests {
    use super::TelemetryBuilder;
    use crate::error::TelemetryError;

    #[test]
    fn stdout_filter_defaults_to_info() {
        let builder = TelemetryBuilder::new("controller");
        assert_eq!(builder.resolve_stdout_filter(), "info");
    }

    #[test]
    fn stdout_filter_prefers_environment_override() {
        let builder = TelemetryBuilder::new("controller")
            .with_env_var("TELEMETRY_TEST_RUST_LOG")
            .with_env_lookup(|name| {
                (name == "TELEMETRY_TEST_RUST_LOG").then(|| "controller=debug".to_string())
            })
            .with_stdout_filter("info");

        assert_eq!(builder.resolve_stdout_filter(), "controller=debug");
    }

    #[cfg(not(feature = "journald"))]
    #[test]
    fn init_rejects_journald_without_feature() {
        let result = TelemetryBuilder::new("controller").enable_journald().init();

        assert!(matches!(
            result,
            Err(TelemetryError::JournaldFeatureDisabled)
        ));
    }

    #[cfg(not(feature = "tokio-metrics"))]
    #[test]
    fn init_rejects_tokio_metrics_without_feature() {
        let result = TelemetryBuilder::new("controller")
            .enable_tokio_metrics()
            .init();

        assert!(matches!(
            result,
            Err(TelemetryError::TokioMetricsFeatureDisabled)
        ));
    }

    #[cfg(all(feature = "tokio-metrics", feature = "otlp"))]
    #[test]
    fn init_rejects_tokio_metrics_without_otlp_config() {
        let result = TelemetryBuilder::new("controller")
            .enable_tokio_metrics()
            .init();

        assert!(matches!(
            result,
            Err(TelemetryError::TokioMetricsRequiresOtlp)
        ));
    }

    #[cfg(all(feature = "tokio-metrics", not(feature = "otlp")))]
    #[test]
    fn init_rejects_tokio_metrics_when_otlp_feature_is_absent() {
        let result = TelemetryBuilder::new("controller")
            .enable_tokio_metrics()
            .init();

        assert!(matches!(
            result,
            Err(TelemetryError::TokioMetricsRequiresOtlp)
        ));
    }
}
