// SPDX-License-Identifier: MIT

//! OTLP/OpenTelemetry export configuration.
//!
//! Enable the `otlp` feature to export traces, logs, and metrics over OTLP/HTTP.
//! Services typically deserialize an [`OtlpConfig`] from their own config file
//! and pass it to [`TelemetryBuilder::with_otlp_config`](crate::TelemetryBuilder::with_otlp_config).
//!
//! # TOML shape
//!
//! ```toml
//! url = "http://localhost:4318"
//! log_level = "info"
//! service_name = "controller"
//! log_rate_limit_per_sec = 100
//! metrics_interval = 5
//!
//! [headers.common]
//! X-Greptime-DB-Name = "edge"
//!
//! [headers.traces]
//! x-greptime-pipeline-name = "greptime_trace_v1"
//!
//! [headers.logs]
//! x-greptime-pipeline-name = "greptime_identity"
//!
//! [headers.metrics]
//! ```
//!
//! `url` is the OTLP/HTTP base URL. When it does not already include a signal
//! suffix, the crate appends `/v1/traces`, `/v1/logs`, and `/v1/metrics` for
//! individual requests. `log_level` applies only to the OTLP trace and log
//! export layers; local stdout and journald filters are configured on
//! [`TelemetryBuilder`](crate::TelemetryBuilder).
//!
//! `headers.common` is sent with every OTLP signal. Signal-specific headers in
//! `headers.traces`, `headers.logs`, and `headers.metrics` override common
//! headers with the same name.
//!
//! # GreptimeDB
//!
//! GreptimeDB export does not require a dedicated crate feature. The crate does
//! not inject GreptimeDB database or pipeline headers automatically; configure
//! every required header explicitly in [`OtlpConfig::headers`]. See
//! `examples/greptime_otlp.toml` and `examples/greptime_otlp.rs` for a
//! compilable loading example.

mod config;
mod endpoint;
mod providers;
mod rate_limit;

pub use config::{OtlpConfig, OtlpHeadersConfig};

pub(crate) use providers::{BuiltProviders, build_providers};
pub(crate) use rate_limit::RateLimitFilter;
