// SPDX-License-Identifier: MIT

use tokio_util::sync::CancellationToken;

use super::config::LogControlConfig;
use super::reload::ReloadState;
use super::router::build_router;
use crate::error::TelemetryError;

/// Starts the localhost-only HTTP log-control server described by `config`.
///
/// The server runs until `cancel_token` is cancelled. The returned join handle
/// tracks the background task lifetime.
///
/// # Arguments
///
/// * `config` - Bind address and port for the HTTP server.
/// * `state` - Shared reload state for filter management.
/// * `cancel_token` - Token that signals the server to shut down gracefully.
///
/// # Returns
///
/// A join handle for the spawned server task.
///
/// # Errors
///
/// Returns [`TelemetryError::LogControl`] if the TCP listener cannot be bound.
pub(crate) fn spawn_log_control_server(
    config: LogControlConfig,
    state: ReloadState,
    cancel_token: CancellationToken,
) -> Result<tokio::task::JoinHandle<()>, TelemetryError> {
    let listener = bind_listener(config.port)?;
    Ok(spawn_log_control_server_from_listener(
        listener,
        state,
        cancel_token,
    ))
}

/// Binds the localhost TCP listener used by the log-control HTTP server.
///
/// # Arguments
///
/// * `port` - TCP port bound on `127.0.0.1`.
///
/// # Returns
///
/// A Tokio TCP listener in nonblocking mode.
///
/// # Errors
///
/// Returns [`TelemetryError::LogControl`] if binding or listener conversion fails.
fn bind_listener(port: u16) -> Result<tokio::net::TcpListener, TelemetryError> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .map_err(TelemetryError::log_control)?;
    listener
        .set_nonblocking(true)
        .map_err(TelemetryError::log_control)?;
    tokio::net::TcpListener::from_std(listener).map_err(TelemetryError::log_control)
}

/// Spawns the log-control HTTP server from an already-bound `listener`.
///
/// # Arguments
///
/// * `listener` - Bound localhost listener accepting HTTP connections.
/// * `state` - Shared reload state for filter management.
/// * `cancel_token` - Token that signals the server to shut down gracefully.
///
/// # Returns
///
/// A join handle for the spawned server task.
fn spawn_log_control_server_from_listener(
    listener: tokio::net::TcpListener,
    state: ReloadState,
    cancel_token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let app = build_router(state);

    tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            cancel_token.cancelled().await;
        });
        if let Err(error) = server.await {
            tracing::error!(error = %error, "log-control server exited with an error");
        }
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio_util::sync::CancellationToken;

    use super::{bind_listener, spawn_log_control_server, spawn_log_control_server_from_listener};
    use crate::error::TelemetryError;
    use crate::log_control::config::LogControlConfig;
    use crate::log_control::reload::ReloadState;

    #[tokio::test]
    async fn spawn_log_control_server_serves_requests_and_shuts_down_on_cancel() {
        let std_listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = std_listener.local_addr().unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();

        let reload_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let callback_calls = reload_calls.clone();
        let state = ReloadState::new(
            "info".to_string(),
            None,
            std::sync::Arc::new(move |_| {
                callback_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            None,
        );
        let cancel_token = CancellationToken::new();
        let handle = spawn_log_control_server_from_listener(listener, state, cancel_token.clone());

        let response = tokio::task::spawn_blocking(move || {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream
                .write_all(
                    b"PUT /filters/stdout HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\ncontent-length: 18\r\nconnection: close\r\n\r\n{\"filter\":\"debug\"}",
                )
                .unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).unwrap();
            String::from_utf8(response).unwrap()
        })
        .await
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("\"stdout\":\"debug\""));
        assert_eq!(reload_calls.load(Ordering::SeqCst), 1);

        cancel_token.cancel();
        handle.await.unwrap();

        let reconnect = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            tokio::net::TcpStream::connect(address),
        )
        .await;
        assert!(reconnect.is_err() || reconnect.unwrap().is_err());
    }

    #[tokio::test]
    async fn bind_listener_reports_log_control_errors_for_in_use_port() {
        let occupied = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();

        let error = bind_listener(port).unwrap_err();

        assert!(matches!(error, TelemetryError::LogControl(_)));
    }

    #[tokio::test]
    async fn spawn_log_control_server_reports_bind_errors() {
        let occupied = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let state = ReloadState::new(
            "info".to_string(),
            None,
            std::sync::Arc::new(|_| Ok(())),
            None,
        );

        let error =
            spawn_log_control_server(LogControlConfig { port }, state, CancellationToken::new())
                .unwrap_err();

        assert!(matches!(error, TelemetryError::LogControl(_)));
    }
}
