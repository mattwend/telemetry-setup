// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Opinionated telemetry primitives for Rust services.
//!
//! The crate is configured explicitly by the service that owns the process. It
//! does not discover or load repository-local config files automatically.
//! Initialize telemetry with [`TelemetryBuilder`] and keep the returned
//! [`TelemetryGuard`] alive for the process lifetime. For graceful teardown,
//! call [`TelemetryGuard::shutdown`] before the runtime exits. Dropping the
//! guard without explicit shutdown is best-effort: on a multi-thread Tokio
//! runtime, OTLP providers are flushed through `tokio::task::block_in_place`,
//! while on a `current_thread` runtime drop may skip the final OTLP flush.
//!
//! # Stdout-only setup
//!
//! [`TelemetryBuilder::new`] enables formatted local [`tracing`] output to
//! stdout by default and leaves optional exporters and control APIs disabled.
//!
//! ```no_run
//! use telemetry_setup::TelemetryBuilder;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut telemetry = TelemetryBuilder::new("controller").init()?;
//! tracing::info!("started");
//! telemetry.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! The stdout filter is read from `RUST_LOG` and falls back to `info`. Use
//! [`TelemetryBuilder::with_stdout_filter`] for a different fallback or
//! [`TelemetryBuilder::with_env_var`] to select a different environment
//! variable. Use [`TelemetryBuilder::without_env_var`] to ignore the process
//! environment.
//!
//! # Recommended integration pattern
//!
//! 1. Decide which crate features the service needs.
//! 2. Load any service-owned config files during service startup.
//! 3. Deserialize telemetry sections into the matching config structs.
//! 4. Pass those structs to [`TelemetryBuilder`].
//! 5. Keep the returned [`TelemetryGuard`] alive until shutdown.
//!
//! Feature-specific config types and builder methods are exported only when the
//! matching feature is enabled: `otlp` exposes `OtlpConfig` and
//! `TelemetryBuilder::with_otlp_config`, and `log-control` exposes
//! `LogControlConfig` and `TelemetryBuilder::with_log_control`.
//!
//! # Optional features
//!
//! - `otlp`: OTLP trace, log, and metric export.
//! - `journald`: `tracing-journald` output.
//! - `log-control`: localhost-only runtime filter update endpoints.
//! - `tokio-metrics`: Tokio runtime gauges recorded through the OpenTelemetry global meter.
//!
//! # Prerequisites
//!
//! Tokio runtime metrics are recorded through the OpenTelemetry global meter.
//! This crate installs a global meter provider when `otlp` is enabled. Without
//! `otlp`, consumers must install their own global meter provider or the metrics
//! will be recorded into OpenTelemetry's default no-op meter.
//!
//! # Examples
//!
//! Compilable example applications live in the repository `examples/`
//! directory. The `greptime_otlp` example requires the `otlp` feature.

mod builder;
mod error;
mod guard;
#[cfg(feature = "log-control")]
#[cfg_attr(docsrs, doc(cfg(feature = "log-control")))]
mod log_control;
#[cfg(feature = "otlp")]
#[cfg_attr(docsrs, doc(cfg(feature = "otlp")))]
mod otlp;
#[cfg(feature = "tokio-metrics")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio-metrics")))]
mod tokio_metrics;

pub use builder::TelemetryBuilder;
pub use error::TelemetryError;
pub use guard::TelemetryGuard;
#[cfg(feature = "log-control")]
#[cfg_attr(docsrs, doc(cfg(feature = "log-control")))]
pub use log_control::LogControlConfig;
#[cfg(feature = "otlp")]
#[cfg_attr(docsrs, doc(cfg(feature = "otlp")))]
pub use otlp::{OtlpConfig, OtlpHeadersConfig};
