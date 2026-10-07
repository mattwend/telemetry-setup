// SPDX-License-Identifier: MIT

use std::sync::Mutex;

use crate::reload::{ReloadCallback, SharedFilter, SharedOtlpFilter};

#[derive(Clone)]
/// Shared state used by the runtime log-control HTTP server.
pub(crate) struct ReloadState {
    /// The currently active local stdout and journald filter string, shared
    /// with a late configuration.
    pub stdout_filter: SharedFilter,
    /// The OTLP filter domain, shared with a late configuration that starts
    /// OTLP export.
    pub otlp_filter: SharedOtlpFilter,
    /// Callback used to reload the local stdout and journald filter domain.
    pub stdout_reload: ReloadCallback,
}

impl ReloadState {
    /// Builds shared reload state from current filter values and reload callbacks.
    ///
    /// # Arguments
    ///
    /// * `stdout_filter` - Shared record of the active local stdout and
    ///   journald filter.
    /// * `otlp_filter` - Shared record of the OTLP filter domain.
    /// * `stdout_reload` - Callback that applies local filter updates.
    ///
    /// # Returns
    ///
    /// A reload state value ready to share with the log-control router.
    pub(crate) fn new(
        stdout_filter: SharedFilter,
        otlp_filter: SharedOtlpFilter,
        stdout_reload: ReloadCallback,
    ) -> Self {
        Self {
            stdout_filter,
            otlp_filter,
            stdout_reload,
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
