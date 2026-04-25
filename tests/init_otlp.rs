// SPDX-License-Identifier: MIT

// This test must live in `tests/` instead of a unit test module because
// `TelemetryBuilder::init()` installs a process-global tracing subscriber.
// Successful initialization can only happen once per process, so an
// integration test gives this case its own fresh test binary.
use std::net::TcpListener;

use telemetry_setup::{OtlpConfig, TelemetryBuilder};

fn unused_local_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("read local addr");
    drop(listener);
    format!("http://{addr}")
}

#[tokio::test]
async fn otlp_init_and_shutdown_succeeds_against_unavailable_collector() {
    let mut guard = TelemetryBuilder::new("test-otlp")
        .without_env_var()
        .with_stdout_filter("info")
        .with_otlp_config(OtlpConfig {
            url: unused_local_url(),
            ..OtlpConfig::default()
        })
        .init()
        .expect("OTLP init should succeed even with unreachable collector");

    tracing::info!("test event");

    guard
        .shutdown()
        .await
        .expect("OTLP shutdown should succeed");
}
