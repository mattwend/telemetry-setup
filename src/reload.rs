// SPDX-License-Identifier: MIT

//! Filter reload callbacks shared by runtime log control and late configuration.
//!
//! Every filter the builder installs is wrapped in a
//! [`tracing_subscriber::reload::Layer`], so a filter can be replaced after the
//! subscriber is installed. The callbacks here hide the subscriber type behind a
//! plain function of the new filter expression.

use std::sync::{Arc, Mutex};

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::reload;

use crate::error::TelemetryError;

/// Replaces one installed filter with a new `EnvFilter` expression.
///
/// Returns the parse or reload error as text, which is what the log-control
/// HTTP API reports.
pub(crate) type ReloadCallback = Arc<dyn Fn(String) -> Result<(), String> + Send + Sync>;

/// The currently active expression of one filter domain, shared by every path
/// that can replace it so that each reports what is actually installed.
pub(crate) type SharedFilter = Arc<Mutex<String>>;

/// Creates the shared record of a filter domain.
///
/// # Arguments
///
/// * `spec` - The expression installed at init.
///
/// # Returns
///
/// A shared, lockable copy of `spec`.
pub(crate) fn shared_filter(spec: impl Into<String>) -> SharedFilter {
    Arc::new(Mutex::new(spec.into()))
}

/// Reloads a filter domain and records the new expression, holding the lock
/// across both so concurrent updates cannot leave the record stale.
///
/// A poisoned lock is recovered: the recorded string stays valid either way.
///
/// # Arguments
///
/// * `current` - The shared record of the domain's active expression.
/// * `reload` - Callback that installs a new expression.
/// * `spec` - The new expression.
///
/// # Returns
///
/// The expression now in effect.
///
/// # Errors
///
/// Returns the callback's error text; the record is then left unchanged.
pub(crate) fn reload_shared(
    current: &Mutex<String>,
    reload: &ReloadCallback,
    spec: String,
) -> Result<String, String> {
    let mut current = current
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reload(spec.clone())?;
    *current = spec;
    Ok(current.clone())
}

/// An OTLP filter domain that is installed and exporting: its active
/// expression and the callback that replaces it.
#[cfg(any(feature = "otlp", feature = "log-control"))]
#[derive(Clone)]
#[cfg_attr(not(feature = "log-control"), allow(dead_code))]
pub(crate) struct OtlpFilter {
    /// The active expression of the OTLP trace and log filters.
    pub current: String,
    /// Reloads the OTLP trace and log filters.
    pub reload: ReloadCallback,
}

#[cfg(any(feature = "otlp", feature = "log-control"))]
impl std::fmt::Debug for OtlpFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtlpFilter")
            .field("current", &self.current)
            .finish_non_exhaustive()
    }
}

/// The OTLP filter domain, absent until OTLP export runs. A late OTLP
/// configuration fills it, so log control can manage it from then on.
#[cfg(any(feature = "otlp", feature = "log-control"))]
pub(crate) type SharedOtlpFilter = Arc<Mutex<Option<OtlpFilter>>>;

/// Creates the shared record of the OTLP filter domain.
///
/// # Arguments
///
/// * `filter` - The domain installed at init, or `None` when OTLP export is
///   not running yet.
///
/// # Returns
///
/// A shared, lockable copy of `filter`.
#[cfg(any(feature = "otlp", feature = "log-control"))]
pub(crate) fn shared_otlp_filter(filter: Option<OtlpFilter>) -> SharedOtlpFilter {
    Arc::new(Mutex::new(filter))
}

/// Builds a callback that replaces the filter behind `handle`.
///
/// # Arguments
///
/// * `handle` - Reload handle of an installed `EnvFilter`.
///
/// # Returns
///
/// A callback that parses its argument and swaps it in.
pub(crate) fn reload_callback<S>(handle: reload::Handle<EnvFilter, S>) -> ReloadCallback
where
    S: tracing::Subscriber + Send + Sync + 'static,
{
    Arc::new(move |spec: String| {
        let filter = EnvFilter::try_new(spec.as_str())
            .map_err(|error| TelemetryError::subscriber(error).to_string())?;
        handle
            .reload(filter)
            .map_err(|error| TelemetryError::subscriber(error).to_string())
    })
}

/// Builds a callback that reloads stdout and optional journald filters.
///
/// # Arguments
///
/// * `fmt_reload` - Callback for the stdout formatting layer.
/// * `journald_reload` - Optional callback for the journald layer.
///
/// # Returns
///
/// A callback that applies one expression to the whole local filter domain.
pub(crate) fn stdout_reload_callback(
    fmt_reload: ReloadCallback,
    journald_reload: Option<ReloadCallback>,
) -> ReloadCallback {
    Arc::new(move |spec: String| {
        fmt_reload(spec.clone())?;

        if let Some(reload) = &journald_reload {
            reload(spec)?;
        }

        Ok(())
    })
}

/// Builds a callback that reloads both OTLP trace and log filters.
///
/// # Arguments
///
/// * `trace_reload` - Callback for the OTLP trace layer.
/// * `log_reload` - Callback for the OTLP log layer.
///
/// # Returns
///
/// A callback that applies one expression to both OTLP export layers.
#[cfg(feature = "otlp")]
pub(crate) fn otlp_reload_callback(
    trace_reload: ReloadCallback,
    log_reload: ReloadCallback,
) -> ReloadCallback {
    Arc::new(move |spec: String| {
        trace_reload(spec.clone())?;
        log_reload(spec)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::filter::EnvFilter;

    use super::stdout_reload_callback;

    fn reload_callback() -> super::ReloadCallback {
        let (layer, reload) = crate::builder::layers::build_fmt_layer::<tracing_subscriber::Registry>(
            EnvFilter::new("info"),
        );
        let _leaked_layer: &'static _ = Box::leak(Box::new(layer));
        reload
    }

    #[test]
    fn stdout_reload_callback_reloads_stdout_and_journald_filters() {
        let callback = stdout_reload_callback(reload_callback(), Some(reload_callback()));

        assert!(callback("debug".to_string()).is_ok());
    }

    #[test]
    fn stdout_reload_callback_reports_malformed_filters() {
        let callback = stdout_reload_callback(reload_callback(), Some(reload_callback()));

        let error = callback("[".to_string()).unwrap_err();

        assert!(error.contains("invalid filter directive"));
    }

    #[test]
    fn reload_shared_records_only_successful_updates() {
        let current = super::shared_filter("info");
        let callback = stdout_reload_callback(reload_callback(), None);

        assert_eq!(
            super::reload_shared(&current, &callback, "off".to_string()),
            Ok("off".to_string())
        );
        assert!(super::reload_shared(&current, &callback, "[".to_string()).is_err());
        assert_eq!(*current.lock().unwrap(), "off");
    }

    #[cfg(feature = "otlp")]
    #[test]
    fn otlp_reload_callback_reloads_trace_and_log_filters() {
        let callback = super::otlp_reload_callback(reload_callback(), reload_callback());

        assert!(callback("debug".to_string()).is_ok());
    }

    #[cfg(feature = "otlp")]
    #[test]
    fn otlp_reload_callback_reports_malformed_filters() {
        let callback = super::otlp_reload_callback(reload_callback(), reload_callback());

        let error = callback("[".to_string()).unwrap_err();

        assert!(error.contains("invalid filter directive"));
    }
}
