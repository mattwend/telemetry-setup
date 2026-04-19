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
/// When OTLP is enabled, provider shutdown is blocking. Drop avoids running
/// that blocking shutdown path when the guard is dropped on an active Tokio
/// runtime thread and logs a warning instead, which may skip final OTLP flushes.
/// Prefer calling [`TelemetryGuard::shutdown`] explicitly during teardown.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropShutdownBehavior {
    SkipCompleted,
    SkipOnRuntime,
    ShutdownProviders,
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
        }
    }

    /// Requests graceful shutdown, waits for background tasks to finish, and
    /// then shuts down OTLP providers.
    ///
    /// This is the preferred teardown path because it allows background tasks
    /// to exit cleanly and performs OTLP provider shutdown outside the degraded
    /// drop fallback.
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
    pub async fn shutdown(mut self) -> Result<(), crate::TelemetryError> {
        self.request_shutdown();
        let task_result = self.wait_for_background_tasks().await;
        self.shutdown_providers();
        self.shutdown_completed.store(true, Ordering::Relaxed);
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

    /// Determines how drop should handle OTLP provider shutdown.
    fn drop_shutdown_behavior(&self) -> DropShutdownBehavior {
        if self.shutdown_completed.load(Ordering::Relaxed) {
            return DropShutdownBehavior::SkipCompleted;
        }

        #[cfg(feature = "otlp")]
        if tokio::runtime::Handle::try_current().is_ok() {
            return DropShutdownBehavior::SkipOnRuntime;
        }

        DropShutdownBehavior::ShutdownProviders
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
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        self.request_shutdown();

        for task in self.background_tasks.drain(..) {
            if !task.is_finished() {
                task.abort();
            }
        }

        match self.drop_shutdown_behavior() {
            DropShutdownBehavior::SkipCompleted => {}
            DropShutdownBehavior::SkipOnRuntime => {
                tracing::warn!(
                    "TelemetryGuard dropped before shutdown() completed; skipping blocking OTLP provider shutdown on an active Tokio runtime"
                );
            }
            DropShutdownBehavior::ShutdownProviders => self.shutdown_providers(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    use super::{DropShutdownBehavior, TelemetryGuard};

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

    #[tokio::test]
    async fn drop_on_tokio_runtime_after_shutdown_does_not_abort_completed_task() {
        let cancel_token = CancellationToken::new();
        let (completed_sender, completed_receiver) = oneshot::channel();
        let task_token = cancel_token.child_token();
        let mut guard = TelemetryGuard::new(cancel_token);
        guard.background_tasks.push(tokio::spawn(async move {
            task_token.cancelled().await;
            let _ = completed_sender.send(());
        }));

        let result = guard.shutdown().await;

        assert!(result.is_ok());
        assert!(completed_receiver.await.is_ok());
    }

    #[test]
    fn drop_shutdown_behavior_skips_provider_shutdown_after_graceful_shutdown() {
        let guard = TelemetryGuard::new(CancellationToken::new());
        guard.shutdown_completed.store(true, Ordering::Relaxed);

        assert_eq!(
            guard.drop_shutdown_behavior(),
            DropShutdownBehavior::SkipCompleted
        );
    }

    #[cfg(feature = "otlp")]
    #[tokio::test]
    async fn drop_shutdown_behavior_skips_provider_shutdown_on_tokio_runtime() {
        let guard = TelemetryGuard::new(CancellationToken::new());

        assert_eq!(
            guard.drop_shutdown_behavior(),
            DropShutdownBehavior::SkipOnRuntime
        );
    }

    #[test]
    fn drop_shutdown_behavior_shuts_down_providers_without_runtime() {
        let guard = TelemetryGuard::new(CancellationToken::new());

        assert_eq!(
            guard.drop_shutdown_behavior(),
            DropShutdownBehavior::ShutdownProviders
        );
    }
}
