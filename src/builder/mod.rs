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
#[cfg(all(feature = "log-control", feature = "otlp"))]
use crate::log_control::otlp_reload_callback;
#[cfg(feature = "log-control")]
use crate::log_control::{
    LogControlConfig, ReloadState, spawn_log_control_server, stdout_reload_callback,
};
#[cfg(feature = "otlp")]
use crate::otlp::OtlpConfig;

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
}

#[cfg(feature = "log-control")]
type StdoutReload = crate::log_control::ReloadCallback;
#[cfg(all(feature = "log-control", feature = "otlp"))]
type OtlpReload = Option<crate::log_control::ReloadCallback>;

#[cfg(feature = "otlp")]
struct InitializedOtlpProviders {
    tracer_provider: opentelemetry_sdk::trace::SdkTracerProvider,
    logger_provider: opentelemetry_sdk::logs::SdkLoggerProvider,
    meter_provider: opentelemetry_sdk::metrics::SdkMeterProvider,
}

#[cfg(feature = "log-control")]
struct LogControlParts {
    stdout_reload: StdoutReload,
    otlp_reload: Option<crate::log_control::ReloadCallback>,
}

struct InstalledSubscriber {
    #[cfg(feature = "otlp")]
    providers: Option<InitializedOtlpProviders>,
    #[cfg(feature = "log-control")]
    log_control: LogControlParts,
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

        #[cfg(all(feature = "log-control", feature = "otlp"))]
        let otlp_current = self
            .otlp_config
            .as_ref()
            .map(|config| config.log_level.clone());
        #[cfg(all(feature = "log-control", not(feature = "otlp")))]
        let otlp_current = None;

        let _installed = self.install_subscriber(stdout_filter, stdout_spec.as_str())?;

        let cancel_token = CancellationToken::new();
        #[allow(unused_mut)]
        let mut guard = TelemetryGuard::new(cancel_token);

        #[cfg(feature = "otlp")]
        if let Some(providers) = _installed.providers {
            guard.tracer_provider = Some(providers.tracer_provider);
            guard.logger_provider = Some(providers.logger_provider);
            guard.meter_provider = Some(providers.meter_provider);
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
            let state = ReloadState::new(
                stdout_spec,
                otlp_current,
                _installed.log_control.stdout_reload,
                _installed.log_control.otlp_reload,
            );
            let task = spawn_log_control_server(config, state, guard.cancel_token.child_token())?;
            guard.background_tasks.push(task);
        }

        Ok(guard)
    }

    fn install_subscriber(
        &self,
        stdout_filter: EnvFilter,
        stdout_spec: &str,
    ) -> Result<InstalledSubscriber, TelemetryError> {
        #[cfg(not(feature = "journald"))]
        let _ = stdout_spec;
        if self.enable_journald {
            #[cfg(feature = "journald")]
            {
                self.install_with_journald(stdout_filter, stdout_spec)
            }
            #[cfg(not(feature = "journald"))]
            unreachable!("journald feature checked before subscriber installation")
        } else {
            self.install_without_journald(stdout_filter)
        }
    }

    #[cfg(all(feature = "otlp", feature = "journald"))]
    fn install_with_journald(
        &self,
        stdout_filter: EnvFilter,
        stdout_spec: &str,
    ) -> Result<InstalledSubscriber, TelemetryError> {
        let (fmt_layer, _fmt_reload_or_unit) =
            layers::build_fmt_layer::<tracing_subscriber::Registry>(stdout_filter);
        let subscriber = tracing_subscriber::registry().with(fmt_layer);
        let (journald_layer, _journald_reload_or_unit) =
            layers::build_journald_layer::<_>(stdout_spec)?;
        let subscriber = subscriber.with(journald_layer);

        if let Some(otlp_config) = self.otlp_config.as_ref() {
            let installed_otlp = self.install_otlp_layers(subscriber, otlp_config)?;

            #[cfg(feature = "log-control")]
            let log_control = LogControlParts {
                stdout_reload: stdout_reload_callback(
                    _fmt_reload_or_unit,
                    Some(_journald_reload_or_unit),
                ),
                otlp_reload: installed_otlp.otlp_reload,
            };

            Ok(InstalledSubscriber {
                providers: installed_otlp.providers,
                #[cfg(feature = "log-control")]
                log_control,
            })
        } else {
            subscriber.try_init().map_err(TelemetryError::subscriber)?;

            #[cfg(feature = "log-control")]
            let log_control = LogControlParts {
                stdout_reload: stdout_reload_callback(
                    _fmt_reload_or_unit,
                    Some(_journald_reload_or_unit),
                ),
                otlp_reload: None,
            };

            Ok(InstalledSubscriber {
                providers: None,
                #[cfg(feature = "log-control")]
                log_control,
            })
        }
    }

    #[cfg(all(not(feature = "otlp"), feature = "journald"))]
    fn install_with_journald(
        &self,
        stdout_filter: EnvFilter,
        stdout_spec: &str,
    ) -> Result<InstalledSubscriber, TelemetryError> {
        let (fmt_layer, _fmt_reload_or_unit) =
            layers::build_fmt_layer::<tracing_subscriber::Registry>(stdout_filter);
        let subscriber = tracing_subscriber::registry().with(fmt_layer);
        let (journald_layer, _journald_reload_or_unit) =
            layers::build_journald_layer::<_>(stdout_spec)?;
        subscriber
            .with(journald_layer)
            .try_init()
            .map_err(TelemetryError::subscriber)?;

        #[cfg(feature = "log-control")]
        let log_control = LogControlParts {
            stdout_reload: stdout_reload_callback(
                _fmt_reload_or_unit,
                Some(_journald_reload_or_unit),
            ),
            otlp_reload: None,
        };

        Ok(InstalledSubscriber {
            #[cfg(feature = "otlp")]
            providers: None,
            #[cfg(feature = "log-control")]
            log_control,
        })
    }

    #[cfg(feature = "otlp")]
    fn install_without_journald(
        &self,
        stdout_filter: EnvFilter,
    ) -> Result<InstalledSubscriber, TelemetryError> {
        let (fmt_layer, _fmt_reload_or_unit) =
            layers::build_fmt_layer::<tracing_subscriber::Registry>(stdout_filter);
        let subscriber = tracing_subscriber::registry().with(fmt_layer);

        if let Some(otlp_config) = self.otlp_config.as_ref() {
            let installed_otlp = self.install_otlp_layers(subscriber, otlp_config)?;

            #[cfg(feature = "log-control")]
            let log_control = LogControlParts {
                stdout_reload: stdout_reload_callback(_fmt_reload_or_unit, None),
                otlp_reload: installed_otlp.otlp_reload,
            };

            Ok(InstalledSubscriber {
                providers: installed_otlp.providers,
                #[cfg(feature = "log-control")]
                log_control,
            })
        } else {
            subscriber.try_init().map_err(TelemetryError::subscriber)?;

            #[cfg(feature = "log-control")]
            let log_control = LogControlParts {
                stdout_reload: stdout_reload_callback(_fmt_reload_or_unit, None),
                otlp_reload: None,
            };

            Ok(InstalledSubscriber {
                providers: None,
                #[cfg(feature = "log-control")]
                log_control,
            })
        }
    }

    #[cfg(not(feature = "otlp"))]
    fn install_without_journald(
        &self,
        stdout_filter: EnvFilter,
    ) -> Result<InstalledSubscriber, TelemetryError> {
        let (fmt_layer, _fmt_reload_or_unit) =
            layers::build_fmt_layer::<tracing_subscriber::Registry>(stdout_filter);
        tracing_subscriber::registry()
            .with(fmt_layer)
            .try_init()
            .map_err(TelemetryError::subscriber)?;

        #[cfg(feature = "log-control")]
        let log_control = LogControlParts {
            stdout_reload: stdout_reload_callback(_fmt_reload_or_unit, None),
            otlp_reload: None,
        };

        Ok(InstalledSubscriber {
            #[cfg(feature = "otlp")]
            providers: None,
            #[cfg(feature = "log-control")]
            log_control,
        })
    }

    #[cfg(feature = "otlp")]
    fn install_otlp_layers<S>(
        &self,
        subscriber: S,
        otlp_config: &OtlpConfig,
    ) -> Result<InstalledOtlp, TelemetryError>
    where
        S: tracing::Subscriber
            + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>
            + Send
            + Sync
            + 'static,
    {
        let layers::OtlpLayerParts { providers } =
            layers::build_otlp_parts(&self.service_name, otlp_config)?;

        #[cfg(feature = "log-control")]
        let (trace_layer, trace_reload) = layers::build_otlp_trace_layer::<_>(
            &providers.tracer_provider,
            otlp_config.log_level.as_str(),
        )?;
        #[cfg(not(feature = "log-control"))]
        let trace_layer = layers::build_otlp_trace_layer::<_>(
            &providers.tracer_provider,
            otlp_config.log_level.as_str(),
        )?;
        let subscriber = subscriber.with(trace_layer);
        #[cfg(feature = "log-control")]
        let (log_layer, log_reload) = layers::build_otlp_log_layer::<_>(
            &providers.logger_provider,
            otlp_config.log_level.as_str(),
            otlp_config.log_rate_limit_per_sec,
        )?;
        #[cfg(not(feature = "log-control"))]
        let log_layer = layers::build_otlp_log_layer::<_>(
            &providers.logger_provider,
            otlp_config.log_level.as_str(),
            otlp_config.log_rate_limit_per_sec,
        )?;
        subscriber
            .with(log_layer)
            .try_init()
            .map_err(TelemetryError::subscriber)?;

        Ok(InstalledOtlp {
            providers: Some(InitializedOtlpProviders {
                tracer_provider: providers.tracer_provider,
                logger_provider: providers.logger_provider,
                meter_provider: providers.meter_provider,
            }),
            #[cfg(feature = "log-control")]
            otlp_reload: otlp_reload_callback(Some(trace_reload), Some(log_reload)),
        })
    }

    /// Resolves the local filter from the configured environment variable or fallback string.
    fn resolve_stdout_filter(&self) -> String {
        filter::resolve_stdout_filter(&self.env_var_name, &self.stdout_filter)
    }
}

#[cfg(feature = "otlp")]
struct InstalledOtlp {
    providers: Option<InitializedOtlpProviders>,
    #[cfg(feature = "log-control")]
    otlp_reload: OtlpReload,
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
