// SPDX-License-Identifier: MIT

use std::time::Duration;

use opentelemetry::global;
use tokio_util::sync::CancellationToken;

/// Snapshot of Tokio runtime metrics recorded by the monitor task.
struct TokioRuntimeMetrics {
    /// Number of worker threads owned by the runtime.
    num_workers: u64,
    /// Number of tasks currently alive on the runtime.
    num_alive_tasks: u64,
    /// Depth of the runtime global task queue.
    global_queue_depth: u64,
}

/// Starts a background task that records Tokio runtime gauges every five seconds.
///
/// This monitor must be started from within an active Tokio runtime because it
/// uses [`tokio::runtime::Handle::current()`]. The underlying Tokio runtime
/// metrics APIs also require compiling the process with
/// `RUSTFLAGS="--cfg tokio_unstable"` when the `tokio-metrics` crate feature is
/// enabled.
///
/// The task runs until `cancel_token` is cancelled. The returned join handle
/// tracks the task lifetime.
///
/// # Arguments
///
/// * `cancel_token` - Token that signals the task to stop collecting metrics.
///
/// # Returns
///
/// A join handle for the spawned background task.
pub fn start_tokio_metrics_monitoring(
    cancel_token: CancellationToken,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    start_tokio_metrics_monitoring_with_interval(cancel_token, interval)
}

/// Starts a Tokio metrics task with a caller-provided recording interval.
///
/// This helper has the same runtime and `tokio_unstable` requirements as
/// [`start_tokio_metrics_monitoring`].
///
/// # Arguments
///
/// * `cancel_token` - Token that signals the task to stop collecting metrics.
/// * `interval` - Delay between recording cycles.
///
/// # Returns
///
/// A join handle for the spawned background task.
fn start_tokio_metrics_monitoring_with_interval(
    cancel_token: CancellationToken,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let meter = global::meter("tokio-runtime");
        let workers = meter.u64_gauge("tokio.runtime.num_workers").build();
        let alive_tasks = meter.u64_gauge("tokio.runtime.num_alive_tasks").build();
        let queue_depth = meter.u64_gauge("tokio.runtime.global_queue_depth").build();

        run_tokio_metrics_monitoring(cancel_token, interval, move |metrics| {
            workers.record(metrics.num_workers, &[]);
            alive_tasks.record(metrics.num_alive_tasks, &[]);
            queue_depth.record(metrics.global_queue_depth, &[]);
        })
        .await;
    })
}

/// Runs the Tokio metrics collection loop until cancelled.
///
/// This loop assumes it is executing on a Tokio runtime whose metrics are
/// available via `tokio_unstable`.
///
/// # Arguments
///
/// * `cancel_token` - Token that signals the loop to stop collecting metrics.
/// * `interval` - Delay between recording cycles.
/// * `record` - Callback invoked with each collected metrics snapshot.
///
/// # Returns
///
/// Resolves after `cancel_token` has been cancelled.
async fn run_tokio_metrics_monitoring<F>(
    cancel_token: CancellationToken,
    interval: Duration,
    mut record: F,
) where
    F: FnMut(TokioRuntimeMetrics),
{
    let handle = tokio::runtime::Handle::current();

    loop {
        record(collect_tokio_runtime_metrics(&handle));

        if tokio::time::timeout(interval, cancel_token.cancelled())
            .await
            .is_ok()
        {
            break;
        }
    }
}

/// Collects the current Tokio runtime metrics from `handle`.
///
/// Tokio exposes these runtime metrics only when the process is compiled with
/// `RUSTFLAGS="--cfg tokio_unstable"`.
///
/// # Arguments
///
/// * `handle` - Runtime handle whose metrics should be sampled.
///
/// # Returns
///
/// A snapshot containing worker, alive task, and queue-depth metrics.
fn collect_tokio_runtime_metrics(handle: &tokio::runtime::Handle) -> TokioRuntimeMetrics {
    let metrics = handle.metrics();
    TokioRuntimeMetrics {
        num_workers: metrics.num_workers() as u64,
        num_alive_tasks: metrics.num_alive_tasks() as u64,
        global_queue_depth: metrics.global_queue_depth() as u64,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::{run_tokio_metrics_monitoring, start_tokio_metrics_monitoring_with_interval};
    use tokio_util::sync::CancellationToken;

    #[tokio::test(flavor = "current_thread")]
    async fn tokio_metrics_task_starts_and_can_shut_down_gracefully() {
        let token = CancellationToken::new();
        let handle =
            start_tokio_metrics_monitoring_with_interval(token.clone(), Duration::from_secs(60));
        tokio::task::yield_now().await;
        assert!(!handle.is_finished());
        token.cancel();
        handle.await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tokio_metrics_loop_records_more_than_one_cycle() {
        let token = CancellationToken::new();
        let recordings = Arc::new(AtomicUsize::new(0));
        let task_recordings = recordings.clone();
        let task_token = token.clone();
        let handle = tokio::spawn(async move {
            run_tokio_metrics_monitoring(task_token, Duration::from_millis(1), |_| {
                task_recordings.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        });

        while recordings.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        token.cancel();
        handle.await.unwrap();

        assert!(recordings.load(Ordering::SeqCst) >= 2);
    }
}
