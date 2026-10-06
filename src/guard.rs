// SPDX-License-Identifier: MIT

#[cfg(feature = "otlp")]
use opentelemetry_sdk::logs::SdkLoggerProvider;
#[cfg(feature = "otlp")]
use opentelemetry_sdk::metrics::SdkMeterProvider;
#[cfg(feature = "otlp")]
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Keeps telemetry providers and background tasks alive for the process lifetime.
///
/// Call [`TelemetryGuard::shutdown`] during application teardown to request
/// graceful shutdown, wait for background tasks to finish, and flush OTLP
/// providers. Dropping the guard without calling `shutdown` first performs a
/// best-effort fallback that cancels the token and aborts any remaining tasks.
///
/// When OTLP is enabled, provider shutdown is blocking. During drop, the guard
/// attempts to flush providers via `tokio::task::block_in_place` when dropped on
/// a multi-thread Tokio runtime. On a current-thread runtime that fallback is
/// unavailable, so drop emits a warning to stderr and may skip the final OTLP
/// flush. Prefer calling [`TelemetryGuard::shutdown`] explicitly during teardown.
#[must_use = "keep TelemetryGuard alive for the process lifetime, or call shutdown().await during teardown"]
#[derive(Debug)]
pub struct TelemetryGuard {
    shutdown_completed: AtomicBool,
    #[cfg(feature = "otlp")]
    pub(crate) tracer_provider: Option<SdkTracerProvider>,
    #[cfg(feature = "otlp")]
    pub(crate) logger_provider: Option<SdkLoggerProvider>,
    #[cfg(feature = "otlp")]
    pub(crate) meter_provider: Option<SdkMeterProvider>,
    /// Shared cancellation token used to signal shutdown to all background tasks.
    pub(crate) cancel_token: CancellationToken,
    pub(crate) background_tasks: Vec<tokio::task::JoinHandle<()>>,
    /// What `init()` prepared for a late configuration, until it is applied.
    pub(crate) late: Option<crate::late::LateSlot>,
}

impl TelemetryGuard {
    /// Creates an empty guard populated by [`crate::TelemetryBuilder`].
    ///
    /// `cancel_token` is the root token whose cancellation signals all background
    /// tasks to shut down.
    pub(crate) fn new(cancel_token: CancellationToken) -> Self {
        Self {
            shutdown_completed: AtomicBool::new(false),
            #[cfg(feature = "otlp")]
            tracer_provider: None,
            #[cfg(feature = "otlp")]
            logger_provider: None,
            #[cfg(feature = "otlp")]
            meter_provider: None,
            cancel_token,
            background_tasks: Vec::new(),
            late: None,
        }
    }

    /// Applies a configuration that became known after `init()`, once.
    ///
    /// Requires [`crate::TelemetryBuilder::with_late_configuration`]. The
    /// stdout filter is replaced unless the environment variable chose it at
    /// init. With the `otlp` feature, a late OTLP configuration starts trace,
    /// log, and metric export; the metric pipeline becomes the global meter
    /// provider. Events emitted before this call are not exported.
    ///
    /// # Arguments
    ///
    /// * `config` - The late configuration.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the configuration is in effect.
    ///
    /// # Errors
    ///
    /// Returns [`crate::TelemetryError::LateConfigurationUnavailable`] when no
    /// late configuration was requested, one was already applied, or
    /// [`TelemetryGuard::shutdown`] has started,
    /// [`crate::TelemetryError::OtlpAlreadyConfigured`] for a late OTLP
    /// configuration when OTLP was configured at init, and a filter or exporter
    /// error otherwise. A failed call consumes the slot.
    pub fn apply_late_configuration(
        &mut self,
        config: crate::LateConfiguration,
    ) -> Result<(), crate::TelemetryError> {
        let slot = self
            .late
            .take()
            .ok_or(crate::TelemetryError::LateConfigurationUnavailable)?;
        let _applied = slot.apply(config)?;
        #[cfg(feature = "otlp")]
        if let Some(meter_provider) = _applied.meter_provider {
            self.meter_provider = Some(meter_provider);
        }
        Ok(())
    }

    /// Requests graceful shutdown, waits for background tasks to finish, and
    /// then shuts down OTLP providers.
    ///
    /// This method is idempotent. After the first call starts shutdown, later
    /// calls return `Ok(())` immediately and do not repeat task or provider
    /// teardown.
    ///
    /// # Returns
    ///
    /// `Ok(())` once all tracked background tasks have been awaited and all
    /// configured OTLP providers have been shut down.
    ///
    /// # Errors
    ///
    /// Returns [`crate::TelemetryError::BackgroundTask`] if a background task
    /// fails or is cancelled unexpectedly. Remaining tasks and providers are
    /// still shut down before the error is returned.
    pub async fn shutdown(&mut self) -> Result<(), crate::TelemetryError> {
        if self.shutdown_completed.swap(true, Ordering::Relaxed) {
            return Ok(());
        }

        // Providers built by a later late configuration would escape teardown,
        // because neither a repeated shutdown nor drop shuts providers down
        // again.
        self.late = None;
        self.request_shutdown();
        let task_result = self.wait_for_background_tasks().await;
        self.shutdown_providers();
        task_result.map_err(crate::TelemetryError::background_task)
    }

    /// Cancels the shared token, signaling all background tasks to stop.
    fn request_shutdown(&self) {
        self.cancel_token.cancel();
    }

    /// Waits for all tracked background tasks to complete.
    ///
    /// Returns `Ok(())` when every task exits successfully. If one or more tasks
    /// return join errors, every task is still awaited and the first error is
    /// returned.
    async fn wait_for_background_tasks(&mut self) -> Result<(), tokio::task::JoinError> {
        let tasks = std::mem::take(&mut self.background_tasks);
        let mut first_error = None;
        for task in tasks {
            if let Err(error) = Self::await_task(task).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Awaits `task` and returns its join result.
    ///
    /// Returns `Ok(())` when the task exits successfully and propagates any join
    /// error otherwise.
    async fn await_task(task: JoinHandle<()>) -> Result<(), tokio::task::JoinError> {
        task.await
    }

    /// Shuts down any configured OTLP providers.
    fn shutdown_providers(&mut self) {
        #[cfg(feature = "otlp")]
        {
            if let Some(provider) = self.logger_provider.take()
                && let Err(error) = provider.shutdown()
            {
                tracing::error!(error = %error, "failed to shut down OTLP logger provider");
            }
            if let Some(provider) = self.meter_provider.take()
                && let Err(error) = provider.shutdown()
            {
                tracing::error!(error = %error, "failed to shut down OTLP meter provider");
            }
            if let Some(provider) = self.tracer_provider.take()
                && let Err(error) = provider.shutdown()
            {
                tracing::error!(error = %error, "failed to shut down OTLP tracer provider");
            }
        }
    }

    /// Runs drop-time provider shutdown.
    ///
    /// When dropping on a multi-thread Tokio runtime, provider shutdown is run
    /// inside `tokio::task::block_in_place` so exporter teardown can block
    /// without stalling the async scheduler. Outside a Tokio runtime, shutdown
    /// runs directly. On a current-thread runtime, `block_in_place` panics and
    /// the caller can fall back to a stderr warning.
    fn run_drop_shutdown(shutdown: impl FnOnce()) -> Result<(), ()> {
        if tokio::runtime::Handle::try_current().is_err() {
            shutdown();
            return Ok(());
        }

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tokio::task::block_in_place(shutdown);
        }))
        .map_err(|_| ())
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        self.request_shutdown();

        for task in self.background_tasks.drain(..) {
            if !task.is_finished() {
                task.abort();
            }
        }

        if self.shutdown_completed.load(Ordering::Relaxed) {
            return;
        }

        if Self::run_drop_shutdown(|| self.shutdown_providers()).is_err() {
            // The tracing subscriber may already be tearing down here, so emit
            // this fallback warning directly to stderr instead of through
            // `tracing`.
            eprintln!(
                "telemetry-setup: TelemetryGuard dropped without shutdown() on a current_thread runtime; OTLP providers were not flushed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    use super::TelemetryGuard;

    #[tokio::test]
    async fn shutdown_cancels_token_and_awaits_background_task() {
        let cancel_token = CancellationToken::new();
        let observed_cancel = Arc::new(AtomicBool::new(false));
        let task_observed_cancel = observed_cancel.clone();
        let task_token = cancel_token.child_token();
        let mut guard = TelemetryGuard::new(cancel_token);
        guard.background_tasks.push(tokio::spawn(async move {
            task_token.cancelled().await;
            task_observed_cancel.store(true, Ordering::SeqCst);
        }));

        let result = guard.shutdown().await;

        assert!(result.is_ok());
        assert!(observed_cancel.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn shutdown_is_idempotent() {
        let cancel_token = CancellationToken::new();
        let mut guard = TelemetryGuard::new(cancel_token);

        assert!(guard.shutdown().await.is_ok());
        assert!(guard.shutdown().await.is_ok());
    }

    #[tokio::test]
    async fn late_configuration_is_refused_after_shutdown() {
        let mut guard = TelemetryGuard::new(CancellationToken::new());
        guard.late = Some(crate::late::LateSlot {
            stdout_reload: std::sync::Arc::new(|_| Ok(())),
            stdout_current: crate::reload::shared_filter("info"),
            stdout_from_env: false,
            #[cfg(feature = "otlp")]
            otlp: None,
            #[cfg(feature = "otlp")]
            otlp_current: crate::reload::shared_otlp_filter(None),
        });

        assert!(guard.shutdown().await.is_ok());
        let result = guard.apply_late_configuration(crate::LateConfiguration::new());

        assert!(matches!(
            result,
            Err(crate::TelemetryError::LateConfigurationUnavailable)
        ));
    }

    #[tokio::test]
    async fn shutdown_returns_first_join_error_after_awaiting_remaining_tasks() {
        let cancel_token = CancellationToken::new();
        let task_token = cancel_token.child_token();
        let (completed_sender, completed_receiver) = oneshot::channel();
        let (shutdown_returned_sender, shutdown_returned_receiver) = oneshot::channel();
        let mut guard = TelemetryGuard::new(cancel_token);
        guard.background_tasks.push(tokio::spawn(async {
            panic!("background task failed");
        }));
        guard.background_tasks.push(tokio::spawn(async move {
            task_token.cancelled().await;
            let _ = completed_sender.send(());
        }));

        let shutdown_task = tokio::spawn(async move {
            let result = guard.shutdown().await;
            let _ = shutdown_returned_sender.send(());
            result
        });

        assert!(completed_receiver.await.is_ok());
        assert!(shutdown_returned_receiver.await.is_ok());
        let result = shutdown_task.await.unwrap();

        let error = result.unwrap_err();
        match error {
            crate::TelemetryError::BackgroundTask(background_error) => {
                let join_error = background_error.0;
                assert!(join_error.is_panic());
                assert_eq!(
                    *join_error.into_panic().downcast::<&str>().unwrap(),
                    "background task failed"
                );
            }
            other => panic!("unexpected shutdown error: {other}"),
        }
    }

    #[tokio::test]
    async fn drop_cancels_token_and_aborts_unfinished_tasks() {
        struct DropNotification(Option<oneshot::Sender<()>>);

        impl Drop for DropNotification {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }

        let cancel_token = CancellationToken::new();
        let cancel_observer = cancel_token.clone();
        let (dropped_sender, dropped_receiver) = oneshot::channel();
        let (started_sender, started_receiver) = oneshot::channel();
        let mut guard = TelemetryGuard::new(cancel_token);
        guard.background_tasks.push(tokio::spawn(async move {
            let _notification = DropNotification(Some(dropped_sender));
            let _ = started_sender.send(());
            std::future::pending::<()>().await;
        }));
        started_receiver.await.unwrap();

        drop(guard);

        assert!(cancel_observer.is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(1), dropped_receiver)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_shutdown_uses_block_in_place_on_multi_thread_runtime() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_in_shutdown = ran.clone();

        let result = TelemetryGuard::run_drop_shutdown(move || {
            ran_in_shutdown.store(true, Ordering::SeqCst);
        });

        assert!(result.is_ok());
        assert!(ran.load(Ordering::SeqCst));
    }

    #[test]
    fn drop_shutdown_returns_error_on_current_thread_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let result = runtime.block_on(async { TelemetryGuard::run_drop_shutdown(|| {}) });

        assert!(result.is_err());
    }
}
