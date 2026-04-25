// SPDX-License-Identifier: MIT

use std::sync::{Arc, Mutex};

pub(crate) type ReloadCallback = Arc<dyn Fn(String) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
/// Shared state used by the runtime log-control HTTP server.
pub(crate) struct ReloadState {
    /// The currently active local stdout and journald filter string.
    pub stdout_filter: Arc<Mutex<String>>,
    /// The currently active OTLP filter string, when OTLP export is enabled.
    pub otlp_filter: Arc<Mutex<Option<String>>>,
    /// Callback used to reload the local stdout and journald filter domain.
    pub stdout_reload: ReloadCallback,
    /// Callback used to reload OTLP trace and log filters.
    pub otlp_reload: Option<ReloadCallback>,
}

impl ReloadState {
    /// Builds shared reload state from current filter values and reload callbacks.
    ///
    /// # Arguments
    ///
    /// * `stdout_filter` - Currently active local stdout and journald filter.
    /// * `otlp_filter` - Currently active OTLP filter, when OTLP export is enabled.
    /// * `stdout_reload` - Callback that applies local filter updates.
    /// * `otlp_reload` - Callback that applies OTLP filter updates, when available.
    ///
    /// # Returns
    ///
    /// A reload state value ready to share with the log-control router.
    pub(crate) fn new(
        stdout_filter: String,
        otlp_filter: Option<String>,
        stdout_reload: ReloadCallback,
        otlp_reload: Option<ReloadCallback>,
    ) -> Self {
        Self {
            stdout_filter: Arc::new(Mutex::new(stdout_filter)),
            otlp_filter: Arc::new(Mutex::new(otlp_filter)),
            stdout_reload,
            otlp_reload,
        }
    }
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
/// A callback suitable for [`ReloadState::stdout_reload`].
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
/// * `trace_reload` - Optional callback for the OTLP trace layer.
/// * `log_reload` - Optional callback for the OTLP log layer.
///
/// # Returns
///
/// A callback when both handles are present, or `None` when OTLP filtering is disabled.
#[cfg(feature = "otlp")]
pub(crate) fn otlp_reload_callback(
    trace_reload: Option<ReloadCallback>,
    log_reload: Option<ReloadCallback>,
) -> Option<ReloadCallback> {
    match (trace_reload, log_reload) {
        (Some(trace_reload), Some(log_reload)) => Some(Arc::new(move |spec: String| {
            trace_reload(spec.clone())?;
            log_reload(spec)?;
            Ok(())
        })),
        _ => None,
    }
}

/// Clones the current value from `lock`, tolerating poisoned locks.
pub(super) fn clone_mutex_value<T: Clone>(lock: &Mutex<T>) -> T {
    match lock.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tracing_subscriber::filter::EnvFilter;

    use super::{clone_mutex_value, stdout_reload_callback};

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

    #[cfg(feature = "otlp")]
    #[test]
    fn otlp_reload_callback_reloads_trace_and_log_filters() {
        let callback =
            super::otlp_reload_callback(Some(reload_callback()), Some(reload_callback()))
                .expect("both handles should create callback");

        assert!(callback("debug".to_string()).is_ok());
    }

    #[cfg(feature = "otlp")]
    #[test]
    fn otlp_reload_callback_reports_malformed_filters() {
        let callback =
            super::otlp_reload_callback(Some(reload_callback()), Some(reload_callback()))
                .expect("both handles should create callback");

        let error = callback("[".to_string()).unwrap_err();

        assert!(error.contains("invalid filter directive"));
    }

    #[test]
    fn clone_mutex_value_recovers_value_from_poisoned_lock() {
        let lock = Mutex::new("info".to_string());
        let _ = std::panic::catch_unwind(|| {
            let _guard = lock.lock().unwrap();
            panic!("poison lock");
        });

        assert_eq!(clone_mutex_value(&lock), "info");
    }
}
