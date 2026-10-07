// SPDX-License-Identifier: MIT

//! Configuration that becomes known only after the subscriber is installed.
//!
//! A service may have to emit before it can read its telemetry configuration:
//! a daemon that is provisioned over its own API, for example, logs while it
//! waits for the configuration that names its collector. Such a service asks
//! for a late configuration with
//! [`TelemetryBuilder::with_late_configuration`](crate::TelemetryBuilder::with_late_configuration)
//! and applies it once with
//! [`TelemetryGuard::apply_late_configuration`](crate::TelemetryGuard::apply_late_configuration).

#[cfg(feature = "otlp")]
use crate::OtlpConfig;
use crate::error::TelemetryError;
use crate::reload::{ReloadCallback, SharedFilter, reload_shared};

/// Telemetry settings applied once, after `init()`.
///
/// Build it with [`LateConfiguration::new`] and the `with_*` methods, so code
/// that sets only some fields keeps compiling when a feature adds one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LateConfiguration {
    /// Fallback stdout and journald filter. The configured environment variable
    /// still takes precedence, exactly as at init.
    pub stdout_filter: Option<String>,
    /// OTLP export to start. Absent leaves OTLP off.
    #[cfg(feature = "otlp")]
    #[cfg_attr(docsrs, doc(cfg(feature = "otlp")))]
    pub otlp: Option<OtlpConfig>,
}

impl LateConfiguration {
    /// Creates an empty late configuration, which changes nothing.
    ///
    /// # Returns
    ///
    /// A configuration without a stdout filter and without OTLP export.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the fallback stdout filter.
    ///
    /// # Arguments
    ///
    /// * `filter` - `tracing_subscriber::EnvFilter` expression used when the
    ///   configured environment variable is absent.
    ///
    /// # Returns
    ///
    /// The updated configuration.
    pub fn with_stdout_filter(mut self, filter: impl Into<String>) -> Self {
        self.stdout_filter = Some(filter.into());
        self
    }

    /// Sets the OTLP export to start.
    ///
    /// # Arguments
    ///
    /// * `config` - Collector endpoint, filter, resource, and rate-limit settings.
    ///
    /// # Returns
    ///
    /// The updated configuration.
    #[cfg(feature = "otlp")]
    #[cfg_attr(docsrs, doc(cfg(feature = "otlp")))]
    pub fn with_otlp_config(mut self, config: OtlpConfig) -> Self {
        self.otlp = Some(config);
        self
    }

    /// Checks that this configuration can be applied, without applying it.
    ///
    /// Parses every filter and builds, then drops, every OTLP exporter, so a
    /// service can refuse a configuration before it commits to it rather than
    /// discover the failure in
    /// [`TelemetryGuard::apply_late_configuration`](crate::TelemetryGuard::apply_late_configuration),
    /// which consumes the late slot even when it fails.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::InvalidFilter`] for a filter that does not
    /// parse and the exporter error for an OTLP export that cannot be built.
    pub fn validate(&self) -> Result<(), TelemetryError> {
        if let Some(filter) = &self.stdout_filter {
            check_filter(filter)?;
        }
        #[cfg(feature = "otlp")]
        if let Some(otlp) = &self.otlp {
            check_filter(&otlp.log_level)?;
            crate::otlp::check_exporters(otlp)?;
        }
        Ok(())
    }
}

fn check_filter(filter: &str) -> Result<(), TelemetryError> {
    tracing_subscriber::EnvFilter::try_new(filter)
        .map(drop)
        .map_err(|error| TelemetryError::InvalidFilter {
            filter: filter.to_string(),
            source: Box::new(error),
        })
}

/// What `init()` installed for a late configuration.
pub(crate) struct LateSlot {
    /// Reloads the stdout and journald filters.
    pub stdout_reload: ReloadCallback,
    /// The active stdout filter, shared with log control so that both report
    /// what a late filter installed.
    pub stdout_current: SharedFilter,
    /// Whether the environment variable chose the stdout filter at init; it
    /// keeps precedence over a late fallback.
    pub stdout_from_env: bool,
    /// The deferred OTLP layers, absent when OTLP was configured at init.
    #[cfg(feature = "otlp")]
    pub otlp: Option<crate::otlp::DeferredOtlp>,
    /// The OTLP filter domain, shared with log control, which a late OTLP
    /// configuration fills.
    #[cfg(feature = "otlp")]
    pub otlp_current: crate::reload::SharedOtlpFilter,
}

impl std::fmt::Debug for LateSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("LateSlot");
        debug.field("stdout_from_env", &self.stdout_from_env);
        #[cfg(feature = "otlp")]
        debug.field("otlp_deferred", &self.otlp.is_some());
        debug.finish_non_exhaustive()
    }
}

/// What applying a late configuration started and the guard must keep.
#[derive(Debug, Default)]
pub(crate) struct Applied {
    /// The metric pipeline of an attached OTLP export.
    #[cfg(feature = "otlp")]
    pub meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
}

impl LateSlot {
    /// Applies `config` to the installed subscriber.
    ///
    /// # Arguments
    ///
    /// * `config` - The late configuration.
    ///
    /// # Returns
    ///
    /// The providers the guard must shut down.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::OtlpAlreadyConfigured`] for a late OTLP
    /// configuration when OTLP was configured at init, and the attach or
    /// reload error otherwise. The stdout filter is applied first.
    pub(crate) fn apply(self, config: LateConfiguration) -> Result<Applied, TelemetryError> {
        #[cfg(feature = "otlp")]
        if config.otlp.is_some() && self.otlp.is_none() {
            return Err(TelemetryError::OtlpAlreadyConfigured);
        }
        if let Some(filter) = config.stdout_filter.filter(|_| !self.stdout_from_env) {
            reload_shared(&self.stdout_current, &self.stdout_reload, filter)
                .map_err(TelemetryError::FilterReload)?;
        }
        #[cfg_attr(not(feature = "otlp"), allow(unused_mut))]
        let mut applied = Applied::default();
        #[cfg(feature = "otlp")]
        if let (Some(otlp), Some(deferred)) = (config.otlp, self.otlp) {
            applied.meter_provider = Some(deferred.attach(&otlp, &self.otlp_current)?);
        }
        Ok(applied)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{LateConfiguration, LateSlot};
    use crate::reload::shared_filter;

    fn slot(stdout_from_env: bool) -> LateSlot {
        LateSlot {
            stdout_reload: Arc::new(|_| Ok(())),
            stdout_current: shared_filter("info"),
            stdout_from_env,
            #[cfg(feature = "otlp")]
            otlp: None,
            #[cfg(feature = "otlp")]
            otlp_current: crate::reload::shared_otlp_filter(None),
        }
    }

    #[test]
    fn validate_accepts_a_valid_configuration() {
        let config = LateConfiguration::new().with_stdout_filter("info,controller=debug");
        #[cfg(feature = "otlp")]
        let config = config.with_otlp_config(crate::OtlpConfig::default());

        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_a_malformed_stdout_filter() {
        let error = LateConfiguration::new()
            .with_stdout_filter("[")
            .validate()
            .unwrap_err();

        assert!(matches!(error, crate::TelemetryError::InvalidFilter { .. }));
    }

    #[cfg(feature = "otlp")]
    #[test]
    fn validate_rejects_a_malformed_otlp_filter_and_url() {
        let filter = LateConfiguration::new().with_otlp_config(crate::OtlpConfig {
            log_level: "[".to_string(),
            ..crate::OtlpConfig::default()
        });
        let url = LateConfiguration::new().with_otlp_config(crate::OtlpConfig {
            url: "://not-a-url".to_string(),
            ..crate::OtlpConfig::default()
        });

        assert!(matches!(
            filter.validate(),
            Err(crate::TelemetryError::InvalidFilter { .. })
        ));
        assert!(matches!(
            url.validate(),
            Err(crate::TelemetryError::OtlpEndpoint { .. })
        ));
    }

    #[test]
    fn a_late_stdout_filter_updates_the_shared_record() {
        let slot = slot(false);
        let current = slot.stdout_current.clone();

        assert!(
            slot.apply(LateConfiguration::new().with_stdout_filter("off"))
                .is_ok()
        );

        assert_eq!(*current.lock().unwrap(), "off");
    }

    #[test]
    fn an_environment_filter_keeps_the_shared_record() {
        let slot = slot(true);
        let current = slot.stdout_current.clone();

        assert!(
            slot.apply(LateConfiguration::new().with_stdout_filter("off"))
                .is_ok()
        );

        assert_eq!(*current.lock().unwrap(), "info");
    }
}
