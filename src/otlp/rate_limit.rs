// SPDX-License-Identifier: MIT

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_core::Metadata;
use tracing_core::subscriber::Interest;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Filter};

const RATE_LIMIT_TARGET: &str = "telemetry::otlp::rate_limit";

#[derive(Debug, Clone)]
pub(crate) struct RateLimitFilter {
    limit_per_sec: Option<u64>,
    state: Arc<State>,
}

#[derive(Debug)]
struct State {
    current_second: AtomicU64,
    seen_this_second: AtomicU64,
    warning_emitted: AtomicBool,
    warning_thread_active: AtomicBool,
}

impl RateLimitFilter {
    /// Creates a rate limiter for OTLP log export events.
    ///
    /// When `limit_per_sec` is `None`, the filter allows all events.
    ///
    /// # Arguments
    ///
    /// * `limit_per_sec` - Maximum number of non-internal events allowed during
    ///   each Unix-second window, or `None` to disable limiting.
    ///
    /// # Returns
    ///
    /// A filter initialized for the current Unix-second window.
    pub(crate) fn new_optional(limit_per_sec: Option<u64>) -> Self {
        Self {
            limit_per_sec,
            state: Arc::new(State {
                current_second: AtomicU64::new(current_unix_second()),
                seen_this_second: AtomicU64::new(0),
                warning_emitted: AtomicBool::new(false),
                warning_thread_active: AtomicBool::new(false),
            }),
        }
    }

    /// Returns whether an event described by `metadata` should reach the OTLP log layer.
    fn allow(&self, metadata: &Metadata<'_>) -> bool {
        let Some(limit_per_sec) = self.limit_per_sec else {
            return true;
        };

        if metadata.target() == RATE_LIMIT_TARGET {
            return true;
        }

        let now = current_unix_second();
        let previous = self.state.current_second.load(Ordering::Relaxed);
        if previous != now
            && self
                .state
                .current_second
                .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            // This rollover is intentionally approximate under contention.
            // Threads racing across a second boundary may observe the old
            // counter before the winner resets it, or increment just before the
            // reset happens. The rate limit is a best-effort operational guard,
            // not an exact accounting mechanism.
            self.state.seen_this_second.store(0, Ordering::Relaxed);
            self.state.warning_emitted.store(false, Ordering::Relaxed);
        }

        let seen = self.state.seen_this_second.fetch_add(1, Ordering::Relaxed);
        if seen < limit_per_sec {
            return true;
        }

        if !self.state.warning_emitted.swap(true, Ordering::Relaxed) {
            emit_rate_limit_warning(limit_per_sec, self.state.clone());
        }

        false
    }
}

impl<S> Filter<S> for RateLimitFilter {
    fn enabled(&self, meta: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        self.allow(meta)
    }

    fn callsite_enabled(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }
}

/// Emits a rate-limit warning from a helper thread to avoid recursive use of the filtered layer.
///
/// At most one helper thread is active at a time so sustained overload does not
/// continuously spawn short-lived threads from the hot filter path.
fn emit_rate_limit_warning(limit_per_sec: u64, state: Arc<State>) {
    if state.warning_thread_active.swap(true, Ordering::Relaxed) {
        return;
    }

    let warning_state = state.clone();
    let result = std::thread::Builder::new()
        .name("telemetry-rate-limit-warning".to_string())
        .spawn(move || {
            tracing::warn!(
                target: RATE_LIMIT_TARGET,
                limit_per_sec,
                "dropping OTLP log exports because the per-second rate limit was exceeded"
            );
            warning_state
                .warning_thread_active
                .store(false, Ordering::Relaxed);
        });

    if let Err(error) = result {
        state.warning_thread_active.store(false, Ordering::Relaxed);
        tracing::debug!(
            target: RATE_LIMIT_TARGET,
            error = %error,
            "failed to spawn OTLP rate-limit warning thread"
        );
    }
}

/// Returns the current Unix timestamp rounded down to seconds.
fn current_unix_second() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{RATE_LIMIT_TARGET, RateLimitFilter};
    use tracing_core::callsite::{Callsite, Identifier};
    use tracing_core::field::FieldSet;
    use tracing_core::metadata::Kind;
    use tracing_core::{Level, Metadata};

    struct TestCallsite;

    static TEST_CALLSITE: TestCallsite = TestCallsite;

    impl Callsite for TestCallsite {
        fn set_interest(&self, _: tracing_core::subscriber::Interest) {}

        fn metadata(&self) -> &Metadata<'_> {
            &TEST_METADATA
        }
    }

    static TEST_METADATA: Metadata<'static> = Metadata::new(
        "test_event",
        "test_target",
        Level::INFO,
        Some(file!()),
        Some(line!()),
        Some(module_path!()),
        FieldSet::new(&[], Identifier(&TEST_CALLSITE)),
        Kind::EVENT,
    );

    static WARNING_METADATA: Metadata<'static> = Metadata::new(
        "rate_limit_warning",
        RATE_LIMIT_TARGET,
        Level::WARN,
        Some(file!()),
        Some(line!()),
        Some(module_path!()),
        FieldSet::new(&[], Identifier(&TEST_CALLSITE)),
        Kind::EVENT,
    );

    #[test]
    fn rate_limit_drops_events_after_limit() {
        let filter = RateLimitFilter::new_optional(Some(2));

        assert!(filter.allow(&TEST_METADATA));
        assert!(filter.allow(&TEST_METADATA));
        assert!(!filter.allow(&TEST_METADATA));
    }

    #[test]
    fn rate_limit_allows_internal_warning_target() {
        let filter = RateLimitFilter::new_optional(Some(0));
        assert!(filter.allow(&WARNING_METADATA));
    }

    #[test]
    fn rate_limit_resets_when_second_changes() {
        let filter = RateLimitFilter::new_optional(Some(1));

        assert!(filter.allow(&TEST_METADATA));
        assert!(!filter.allow(&TEST_METADATA));

        filter.state.current_second.store(
            super::current_unix_second().saturating_sub(1),
            std::sync::atomic::Ordering::Relaxed,
        );

        assert!(filter.allow(&TEST_METADATA));
    }
}
