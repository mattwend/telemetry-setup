// SPDX-License-Identifier: MIT

use std::sync::{Arc, Mutex, RwLock};

use tracing_subscriber::filter::EnvFilter;

use crate::builder::layers::FilterReloadHandle;

pub(crate) type ReloadCallback = Arc<dyn Fn(String) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
/// Shared state used by the runtime log-control HTTP server.
pub(crate) struct ReloadState {
    /// The currently active local stdout and journald filter string.
    pub stdout_filter: Arc<RwLock<String>>,
    /// Serializes stdout filter reloads with state updates.
    pub stdout_update_lock: Arc<Mutex<()>>,
    /// The currently active OTLP filter string, when OTLP export is enabled.
    pub otlp_filter: Arc<RwLock<Option<String>>>,
    /// Serializes OTLP filter reloads with state updates.
    pub otlp_update_lock: Arc<Mutex<()>>,
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
            stdout_filter: Arc::new(RwLock::new(stdout_filter)),
            stdout_update_lock: Arc::new(Mutex::new(())),
            otlp_filter: Arc::new(RwLock::new(otlp_filter)),
            otlp_update_lock: Arc::new(Mutex::new(())),
            stdout_reload,
            otlp_reload,
        }
    }
}

/// Builds a callback that reloads stdout and optional journald filters.
///
/// # Arguments
///
/// * `fmt_filter_handle` - Reload handle for the stdout formatting layer.
/// * `journald_reload_handle` - Optional reload handle for the journald layer.
///
/// # Returns
///
/// A callback suitable for [`ReloadState::stdout_reload`].
pub(crate) fn stdout_reload_callback(
    fmt_filter_handle: FilterReloadHandle,
    journald_reload_handle: Option<FilterReloadHandle>,
) -> ReloadCallback {
    Arc::new(move |spec: String| {
        let fmt_filter = EnvFilter::try_new(spec.as_str()).map_err(|error| error.to_string())?;
        let journald_filter = if journald_reload_handle.is_some() {
            Some(EnvFilter::try_new(spec.as_str()).map_err(|error| error.to_string())?)
        } else {
            None
        };

        fmt_filter_handle
            .reload(fmt_filter)
            .map_err(|error| error.to_string())?;

        if let (Some(handle), Some(filter)) = (journald_reload_handle.clone(), journald_filter) {
            handle.reload(filter).map_err(|error| error.to_string())?;
        }

        Ok(())
    })
}

/// Builds a callback that reloads both OTLP trace and log filters.
///
/// # Arguments
///
/// * `trace_handle` - Optional reload handle for the OTLP trace layer.
/// * `log_handle` - Optional reload handle for the OTLP log layer.
///
/// # Returns
///
/// A callback when both handles are present, or `None` when OTLP filtering is disabled.
#[cfg(feature = "otlp")]
pub(crate) fn otlp_reload_callback(
    trace_handle: Option<FilterReloadHandle>,
    log_handle: Option<FilterReloadHandle>,
) -> Option<ReloadCallback> {
    match (trace_handle, log_handle) {
        (Some(trace_handle), Some(log_handle)) => Some(Arc::new(move |spec: String| {
            let trace_filter =
                EnvFilter::try_new(spec.as_str()).map_err(|error| error.to_string())?;
            let log_filter =
                EnvFilter::try_new(spec.as_str()).map_err(|error| error.to_string())?;
            trace_handle
                .reload(trace_filter)
                .map_err(|error| error.to_string())?;
            log_handle
                .reload(log_filter)
                .map_err(|error| error.to_string())?;
            Ok(())
        })),
        _ => None,
    }
}

/// Reads the current value from `lock`, tolerating poisoned locks.
pub(super) fn read_lock<T: Clone>(lock: &RwLock<T>) -> T {
    match lock.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Writes `value` into `lock`, tolerating poisoned locks.
pub(super) fn write_lock<T>(lock: &RwLock<T>, value: T) {
    match lock.write() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => *poisoned.into_inner() = value,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::RwLock;

    use tracing_subscriber::filter::EnvFilter;

    use super::{read_lock, stdout_reload_callback, write_lock};

    fn reload_handle() -> super::FilterReloadHandle {
        let (layer, handle) = crate::builder::layers::build_fmt_layer(EnvFilter::new("info"));
        // The reload handle points back to its layer. Keep the layer alive for
        // the duration of the process so callback tests exercise successful
        // reloads rather than `SubscriberGone` errors.
        let _leaked_layer: &'static _ = Box::leak(layer);
        handle
    }

    #[test]
    fn stdout_reload_callback_reloads_stdout_and_journald_filters() {
        let callback = stdout_reload_callback(reload_handle(), Some(reload_handle()));

        assert!(callback("debug".to_string()).is_ok());
    }

    #[test]
    fn stdout_reload_callback_reports_malformed_filters() {
        let callback = stdout_reload_callback(reload_handle(), Some(reload_handle()));

        let error = callback("[".to_string()).unwrap_err();

        assert!(error.contains("invalid filter directive"));
    }

    #[cfg(feature = "otlp")]
    #[test]
    fn otlp_reload_callback_reloads_trace_and_log_filters() {
        let callback = super::otlp_reload_callback(Some(reload_handle()), Some(reload_handle()))
            .expect("both handles should create callback");

        assert!(callback("debug".to_string()).is_ok());
    }

    #[cfg(feature = "otlp")]
    #[test]
    fn otlp_reload_callback_reports_malformed_filters() {
        let callback = super::otlp_reload_callback(Some(reload_handle()), Some(reload_handle()))
            .expect("both handles should create callback");

        let error = callback("[".to_string()).unwrap_err();

        assert!(error.contains("invalid filter directive"));
    }

    #[test]
    fn read_lock_recovers_value_from_poisoned_lock() {
        let lock = RwLock::new("info".to_string());
        let _ = std::panic::catch_unwind(|| {
            let _guard = lock.write().unwrap();
            panic!("poison lock");
        });

        assert_eq!(read_lock(&lock), "info");
    }

    #[test]
    fn write_lock_recovers_and_updates_poisoned_lock() {
        let lock = RwLock::new("info".to_string());
        let _ = std::panic::catch_unwind(|| {
            let mut guard = lock.write().unwrap();
            *guard = "warn".to_string();
            panic!("poison lock");
        });

        write_lock(&lock, "debug".to_string());

        assert_eq!(read_lock(&lock), "debug");
    }
}
