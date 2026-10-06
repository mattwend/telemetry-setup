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
//!   compiled with `tokio-metrics`.
//! - [`TelemetryBuilder::with_tokio_metrics_interval`] sets the Tokio runtime
//!   metrics sampling cadence.
//! - `with_otlp_config` and `with_log_control` are available behind their
//!   matching crate features.
//!
//! See the repository `examples/` directory for complete binaries.

mod feature_checks;
mod filter;
pub(crate) mod layers;

use std::time::Duration;

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use tokio_util::sync::CancellationToken;

use crate::error::TelemetryError;
use crate::guard::TelemetryGuard;
use crate::late::LateSlot;
#[cfg(feature = "log-control")]
use crate::log_control::{LogControlConfig, ReloadState, spawn_log_control_server};
#[cfg(feature = "otlp")]
use crate::otlp::OtlpConfig;
#[cfg(feature = "otlp")]
use crate::reload::otlp_reload_callback;
#[cfg(any(feature = "otlp", feature = "log-control"))]
use crate::reload::{OtlpFilter, shared_otlp_filter};
use crate::reload::{ReloadCallback, shared_filter, stdout_reload_callback};

/// Builder for installing local logging, optional OTLP export, and related helpers.
pub struct TelemetryBuilder {
    #[cfg_attr(not(feature = "otlp"), allow(dead_code))]
    service_name: String,
    #[cfg(feature = "otlp")]
    otlp_config: Option<OtlpConfig>,
    stdout_filter: Option<String>,
    env_var_name: Option<String>,
    #[cfg(feature = "log-control")]
    log_control_config: Option<LogControlConfig>,
    enable_journald: bool,
    enable_tokio_metrics: bool,
    tokio_metrics_interval: Duration,
    late_configuration: bool,
}

#[cfg(feature = "otlp")]
struct InitializedOtlpProviders {
    tracer_provider: opentelemetry_sdk::trace::SdkTracerProvider,
    logger_provider: opentelemetry_sdk::logs::SdkLoggerProvider,
    meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
}

/// What `install_subscriber` installed.
struct InstalledSubscriber {
    /// Reloads the stdout and journald filters.
    stdout_reload: ReloadCallback,
    /// The OTLP filter domain of an OTLP export configured at init.
    #[cfg(any(feature = "otlp", feature = "log-control"))]
    otlp_filter: Option<OtlpFilter>,
    #[cfg(feature = "otlp")]
    providers: Option<InitializedOtlpProviders>,
    #[cfg(feature = "otlp")]
    deferred: Option<crate::otlp::DeferredOtlp>,
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
        debug.field("tokio_metrics_interval", &self.tokio_metrics_interval);
        debug.field("late_configuration", &self.late_configuration);
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
            #[cfg(feature = "log-control")]
            log_control_config: None,
            enable_journald: false,
            enable_tokio_metrics: false,
            tokio_metrics_interval: Duration::from_secs(5),
            late_configuration: false,
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

    /// Sets the Tokio runtime metrics collection interval.
    ///
    /// # Arguments
    ///
    /// * `interval` - Delay between Tokio runtime metrics snapshots.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn with_tokio_metrics_interval(mut self, interval: Duration) -> Self {
        self.tokio_metrics_interval = interval;
        self
    }

    /// Prepares the subscriber for a configuration that is known only after
    /// `init()`, applied once with
    /// [`TelemetryGuard::apply_late_configuration`](crate::TelemetryGuard::apply_late_configuration).
    ///
    /// The stdout filter stays reloadable. With the `otlp` feature and no OTLP
    /// configuration at init, the OTLP trace and log layers are installed
    /// filtered `off` and without an exporter, so a late OTLP configuration can
    /// start export without a second subscriber. Tokio runtime metrics started
    /// at init keep the meter provider that was global at init.
    ///
    /// # Returns
    ///
    /// The updated builder.
    pub fn with_late_configuration(mut self) -> Self {
        self.late_configuration = true;
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

        let stdout_spec = self.resolve_stdout_filter();
        let stdout_filter =
            EnvFilter::try_new(stdout_spec.as_str()).map_err(TelemetryError::subscriber)?;

        let installed = self.install_subscriber(stdout_filter, stdout_spec.as_str())?;

        let cancel_token = CancellationToken::new();
        let mut guard = TelemetryGuard::new(cancel_token);
        let stdout_current = shared_filter(stdout_spec);
        #[cfg(any(feature = "otlp", feature = "log-control"))]
        let otlp_current = shared_otlp_filter(installed.otlp_filter);

        #[cfg(feature = "otlp")]
        if let Some(providers) = installed.providers {
            guard.tracer_provider = Some(providers.tracer_provider);
            guard.logger_provider = Some(providers.logger_provider);
            guard.meter_provider = providers.meter_provider;
        }

        if self.late_configuration {
            guard.late = Some(LateSlot {
                stdout_reload: installed.stdout_reload.clone(),
                stdout_current: stdout_current.clone(),
                stdout_from_env: filter::environment_filter(&self.env_var_name).is_some(),
                #[cfg(feature = "otlp")]
                otlp: installed.deferred,
                #[cfg(feature = "otlp")]
                otlp_current: otlp_current.clone(),
            });
        }

        #[cfg(feature = "tokio-metrics")]
        if self.enable_tokio_metrics {
            let task = crate::tokio_metrics::start_tokio_metrics_monitoring(
                guard.cancel_token.child_token(),
                self.tokio_metrics_interval,
            );
            guard.background_tasks.push(task);
        }

        #[cfg(feature = "log-control")]
        if let Some(config) = self.log_control_config {
            let state = ReloadState::new(stdout_current, otlp_current, installed.stdout_reload);
            let task = spawn_log_control_server(config, state, guard.cancel_token.child_token())?;
            guard.background_tasks.push(task);
        }

        Ok(guard)
    }

    /// Composes and installs the subscriber: stdout, optional journald, and
    /// optional OTLP trace and log layers, every filter reloadable.
    ///
    /// # Arguments
    ///
    /// * `stdout_filter` - Initial stdout filter.
    /// * `stdout_spec` - The expression `stdout_filter` was parsed from, shared
    ///   with journald.
    ///
    /// # Returns
    ///
    /// The reload callbacks and providers of the installed layers.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError`] when a layer or provider cannot be built or
    /// the subscriber cannot be installed.
    fn install_subscriber(
        &self,
        stdout_filter: EnvFilter,
        stdout_spec: &str,
    ) -> Result<InstalledSubscriber, TelemetryError> {
        let (fmt_layer, fmt_reload) =
            layers::build_fmt_layer::<tracing_subscriber::Registry>(stdout_filter);

        #[cfg(feature = "journald")]
        let (journald_layer, journald_reload) = if self.enable_journald {
            let (layer, reload) = layers::build_journald_layer::<_>(stdout_spec)?;
            (Some(layer), Some(reload))
        } else {
            (None, None)
        };
        #[cfg(not(feature = "journald"))]
        let (journald_layer, journald_reload): (
            Option<tracing_subscriber::layer::Identity>,
            Option<ReloadCallback>,
        ) = {
            let _ = stdout_spec;
            (None, None)
        };
        let stdout_reload = stdout_reload_callback(fmt_reload, journald_reload);
        let subscriber = tracing_subscriber::registry()
            .with(fmt_layer)
            .with(journald_layer);

        #[cfg(feature = "otlp")]
        {
            let otlp = self.build_otlp()?;
            let (trace_layer, log_layer, otlp_filter, providers, deferred) = match otlp {
                Some(otlp) => {
                    let (trace_layer, trace_reload) = layers::build_otlp_trace_layer::<_>(
                        &otlp.tracer_provider,
                        otlp.filter.as_str(),
                    )?;
                    let (log_layer, log_reload) = layers::build_otlp_log_layer::<_>(
                        &otlp.logger_provider,
                        otlp.filter.as_str(),
                        otlp.rate_limit.clone(),
                    )?;
                    let reload = otlp_reload_callback(trace_reload, log_reload);
                    let providers = InitializedOtlpProviders {
                        tracer_provider: otlp.tracer_provider,
                        logger_provider: otlp.logger_provider,
                        meter_provider: otlp.meter_provider,
                    };
                    match otlp.deferred {
                        Some(deferred) => {
                            let deferred = crate::otlp::DeferredOtlp::new(
                                self.service_name.clone(),
                                &deferred,
                                reload,
                                otlp.rate_limit,
                            );
                            (
                                Some(trace_layer),
                                Some(log_layer),
                                None,
                                Some(providers),
                                Some(deferred),
                            )
                        }
                        None => (
                            Some(trace_layer),
                            Some(log_layer),
                            Some(OtlpFilter {
                                current: otlp.filter,
                                reload,
                            }),
                            Some(providers),
                            None,
                        ),
                    }
                }
                None => (None, None, None, None, None),
            };
            subscriber
                .with(trace_layer)
                .with(log_layer)
                .try_init()
                .map_err(TelemetryError::subscriber)?;
            Ok(InstalledSubscriber {
                stdout_reload,
                otlp_filter,
                providers,
                deferred,
            })
        }
        #[cfg(not(feature = "otlp"))]
        {
            subscriber.try_init().map_err(TelemetryError::subscriber)?;
            Ok(InstalledSubscriber {
                stdout_reload,
                #[cfg(feature = "log-control")]
                otlp_filter: None,
            })
        }
    }

    /// Builds the OTLP providers to install: configured ones, deferred ones
    /// for a late configuration, or none.
    ///
    /// # Returns
    ///
    /// The providers and the initial filter and rate limit of their layers.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError`] when configured providers cannot be built.
    #[cfg(feature = "otlp")]
    fn build_otlp(&self) -> Result<Option<OtlpParts>, TelemetryError> {
        if let Some(config) = &self.otlp_config {
            let providers = crate::otlp::build_providers(&self.service_name, config)?;
            return Ok(Some(OtlpParts {
                tracer_provider: providers.tracer_provider,
                logger_provider: providers.logger_provider,
                meter_provider: Some(providers.meter_provider),
                filter: config.log_level.clone(),
                rate_limit: crate::otlp::RateLimitFilter::new_optional(
                    config.log_rate_limit_per_sec,
                ),
                deferred: None,
            }));
        }
        if !self.late_configuration {
            return Ok(None);
        }
        let deferred = crate::otlp::build_deferred_providers();
        Ok(Some(OtlpParts {
            tracer_provider: deferred.tracer_provider.clone(),
            logger_provider: deferred.logger_provider.clone(),
            meter_provider: None,
            filter: crate::otlp::DEFERRED_FILTER.to_string(),
            rate_limit: crate::otlp::RateLimitFilter::new_optional(None),
            deferred: Some(deferred),
        }))
    }

    /// Resolves the local filter from the configured environment variable or fallback string.
    fn resolve_stdout_filter(&self) -> String {
        filter::resolve_stdout_filter(&self.env_var_name, &self.stdout_filter)
    }
}

/// The OTLP providers `init()` installs layers over.
#[cfg(feature = "otlp")]
struct OtlpParts {
    tracer_provider: opentelemetry_sdk::trace::SdkTracerProvider,
    logger_provider: opentelemetry_sdk::logs::SdkLoggerProvider,
    meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
    filter: String,
    rate_limit: crate::otlp::RateLimitFilter,
    deferred: Option<crate::otlp::DeferredProviders>,
}

#[cfg(test)]
mod tests {
    use super::TelemetryBuilder;
    use crate::builder::filter::resolve_stdout_filter_with_lookup;

    #[test]
    fn stdout_filter_defaults_to_info() {
        let builder = TelemetryBuilder::new("controller");
        assert_eq!(builder.resolve_stdout_filter(), "info");
    }

    #[test]
    fn stdout_filter_prefers_environment_override() {
        let env_var_name = Some("TELEMETRY_TEST_RUST_LOG".to_string());
        let stdout_filter = Some("info".to_string());

        assert_eq!(
            resolve_stdout_filter_with_lookup(
                &env_var_name,
                |name| {
                    (name == "TELEMETRY_TEST_RUST_LOG").then(|| "controller=debug".to_string())
                },
                &stdout_filter,
            ),
            "controller=debug"
        );
    }

    #[cfg(not(feature = "journald"))]
    #[test]
    fn init_rejects_journald_without_feature() {
        let result = TelemetryBuilder::new("controller").enable_journald().init();

        assert!(matches!(
            result,
            Err(crate::TelemetryError::JournaldFeatureDisabled)
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
            Err(crate::TelemetryError::TokioMetricsFeatureDisabled)
        ));
    }
}
