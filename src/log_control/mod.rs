// SPDX-License-Identifier: MIT

//! Local runtime filter control API.
//!
//! Enable the `log-control` feature and pass a [`LogControlConfig`] to
//! `TelemetryBuilder::with_log_control` to bind a localhost-only HTTP server on
//! `127.0.0.1`.
//!
//! The server has no authentication and accepts connections only from the local
//! machine. Any local process that can reach `127.0.0.1` on the configured port
//! can inspect and change runtime filters.
//!
//! Available endpoints:
//!
//! - `GET /filters` returns `{ "stdout": "...", "otlp": "..." | null }`
//! - `PUT /filters/stdout` accepts `{ "filter": "..." }` and returns the
//!   updated filter state or `400 Bad Request`
//! - `PUT /filters/otlp` accepts `{ "filter": "..." }` and returns the updated
//!   filter state, `400 Bad Request`, or `404 Not Found` when OTLP is
//!   unavailable for the process
//!
//! `PUT /filters/otlp` updates OTLP trace and log filters only when OTLP is
//! enabled for the process.

mod config;
mod reload;
mod router;
mod server;

pub use config::LogControlConfig;

#[cfg(feature = "otlp")]
pub(crate) use reload::otlp_reload_callback;
pub(crate) use reload::{ReloadCallback, ReloadState, stdout_reload_callback};
pub(crate) use server::spawn_log_control_server;
