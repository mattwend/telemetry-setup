// SPDX-License-Identifier: MIT

// This test must live in `tests/` instead of a unit test module because
// `TelemetryBuilder::init()` installs a process-global tracing subscriber.
// Successful initialization can only happen once per process, so an
// integration test gives this case its own fresh test binary.
use telemetry_setup::TelemetryBuilder;

#[tokio::test]
async fn stdout_only_init_succeeds_and_returns_shutdown_guard() {
    let mut guard = TelemetryBuilder::new("controller")
        .without_env_var()
        .with_stdout_filter("info")
        .init()
        .expect("stdout-only telemetry initialization should succeed");

    guard
        .shutdown()
        .await
        .expect("stdout-only guard shutdown should succeed");
}
