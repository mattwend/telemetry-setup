// SPDX-License-Identifier: MIT

use std::sync::{Arc, Mutex};

use crate::reload::{ReloadCallback, SharedFilter};

#[derive(Clone)]
/// Shared state used by the runtime log-control HTTP server.
pub(crate) struct ReloadState {
    /// The currently active local stdout and journald filter string, shared
    /// with a late configuration.
    pub stdout_filter: SharedFilter,
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
    /// * `stdout_filter` - Shared record of the active local stdout and
    ///   journald filter.
    /// * `otlp_filter` - Currently active OTLP filter, when OTLP export is enabled.
    /// * `stdout_reload` - Callback that applies local filter updates.
    /// * `otlp_reload` - Callback that applies OTLP filter updates, when available.
    ///
    /// # Returns
    ///
    /// A reload state value ready to share with the log-control router.
    pub(crate) fn new(
        stdout_filter: SharedFilter,
        otlp_filter: Option<String>,
        stdout_reload: ReloadCallback,
        otlp_reload: Option<ReloadCallback>,
    ) -> Self {
        Self {
            stdout_filter,
            otlp_filter: Arc::new(Mutex::new(otlp_filter)),
            stdout_reload,
            otlp_reload,
        }
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

    use super::clone_mutex_value;

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
