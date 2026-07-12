# telemetry-setup

[![CI](https://github.com/mattwend/telemetry-setup/actions/workflows/ci.yml/badge.svg)](https://github.com/mattwend/telemetry-setup/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/mattwend/telemetry-setup/branch/v0.x.x/graph/badge.svg)](https://codecov.io/gh/mattwend/telemetry-setup)

Opinionated telemetry setup for Rust services.

## Quick start

```rust
use telemetry_setup::TelemetryBuilder;

fn main() -> Result<(), telemetry_setup::TelemetryError> {
    let _telemetry = TelemetryBuilder::new("controller").init()?;

    tracing::info!("started");
    Ok(())
}
```

Keep the returned `TelemetryGuard` alive for the lifetime of the process:

- call `TelemetryGuard::shutdown().await` during teardown for a clean flush
- initialize telemetry once per process; calling `init()` more than once is unsupported
- dropping the guard without calling `shutdown()` first is only a best-effort fallback
- on multi-thread Tokio runtimes, drop attempts a final OTLP flush
- on `current_thread` runtimes, OTLP flush-on-drop is unavailable because that path relies on `tokio::task::block_in_place`, so explicit shutdown is strongly preferred

This crate enables Tokio's `rt-multi-thread` feature to support the multi-thread drop path.

## Prerequisites

- Rust 1.87 or newer.

Tokio metrics require the `tokio-metrics` feature and an installed OpenTelemetry
meter provider. With OTLP enabled, this crate installs one automatically.
Without OTLP, consumers must install their own meter provider or Tokio metrics
will record into OpenTelemetry's default no-op global meter.

## Features

- formatted `tracing` logs to stdout
- optional OTLP export for traces, logs, and metrics
- optional `journald` output
- optional localhost-only log-level control API
- optional Tokio runtime metrics via OpenTelemetry

Cargo features:

- `otlp` — enables OTLP trace, log, and metric export and the OTLP configuration types
- `journald` — enables `tracing-journald` output
- `log-control` — enables localhost-only HTTP endpoints for runtime filter changes
- `tokio-metrics` — enables Tokio runtime gauges via OpenTelemetry

## Full configuration example

This example assumes the `otlp`, `log-control`, `journald`, and `tokio-metrics`
crate features are enabled.

```rust
# use std::collections::HashMap;
# use telemetry_setup::{LogControlConfig, OtlpConfig, OtlpHeadersConfig, TelemetryBuilder};
# fn example() -> Result<(), telemetry_setup::TelemetryError> {
let _telemetry = TelemetryBuilder::new("controller")
    .with_stdout_filter("info")
    .with_otlp_config(OtlpConfig {
        url: "http://localhost:4318".to_string(),
        headers: OtlpHeadersConfig {
            common: HashMap::from([(
                "X-Greptime-DB-Name".to_string(),
                "edge".to_string(),
            )]),
            traces: HashMap::from([(
                "x-greptime-pipeline-name".to_string(),
                "greptime_trace_v1".to_string(),
            )]),
            logs: HashMap::from([(
                "x-greptime-pipeline-name".to_string(),
                "greptime_identity".to_string(),
            )]),
            ..OtlpHeadersConfig::default()
        },
        ..OtlpConfig::default()
    })
    .with_log_control(LogControlConfig { port: 6669 })
    .enable_journald()
    .enable_tokio_metrics()
    .init()?;
# Ok(())
# }
```

Defaults:

- stdout filter: `RUST_LOG`, falling back to `info`
- log-control port: `6669`
- OTLP endpoint: `http://localhost:4318`
- OTLP metric interval: 5 seconds, configurable from 1 second to 1 day
- Tokio metrics interval: 5 seconds

Use `TelemetryBuilder::without_env_var()` to ignore `RUST_LOG`.

## Log control API

When `log-control` is enabled, the crate binds an HTTP server only on `127.0.0.1`.
There is no authentication, so any local process can inspect and change runtime filters.
Invalid filters return `400`.

- `GET /filters` returns `{ "stdout": "...", "otlp": "..." | null }`
- `PUT /filters/stdout` accepts `{ "filter": "..." }` and returns the updated filter state
- `PUT /filters/otlp` accepts `{ "filter": "..." }`, returns the updated filter state, and returns `404` when OTLP is unavailable

## Examples

See `examples/`:

- `stdout_only.rs` — local stdout logging with no exporters or control API
- `stdout_custom_filter.rs` — stdout logging with an explicit fallback filter
- `greptime_otlp.rs` — GreptimeDB OTLP/HTTP traces, logs, and metrics configuration

Companion configuration:

- `greptime_otlp.toml` — configuration for the GreptimeDB example

GreptimeDB export does not require a dedicated feature; configure headers with
`OtlpConfig::headers`.

## License

Licensed under the MIT License. See [LICENSE](LICENSE).
