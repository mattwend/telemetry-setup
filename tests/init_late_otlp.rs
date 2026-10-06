// SPDX-License-Identifier: MIT

// `init()` installs a process-global subscriber, so this case needs its own
// test binary.
use std::sync::{Arc, Mutex};

use telemetry_setup::{LateConfiguration, OtlpConfig, TelemetryBuilder, TelemetryError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One received OTLP/HTTP request: its path and its raw body.
type Received = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// A collector that records every request and answers `200 OK`.
async fn collector() -> (String, Received) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let received = Received::default();
    let sink = received.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 8192];
                loop {
                    let Ok(read) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buffer[..end]).to_string();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if buffer.len() < end + 4 + length {
                        continue;
                    }
                    let path = head
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let body = buffer[end + 4..end + 4 + length].to_vec();
                    buffer.drain(..end + 4 + length);
                    sink.lock().expect("sink").push((path, body));
                    let response = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n";
                    if stream.write_all(response).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (url, received)
}

fn log_bodies_contain(received: &Received, needle: &str) -> bool {
    received
        .lock()
        .expect("sink")
        .iter()
        .filter(|(path, _)| path == "/v1/logs")
        .any(|(_, body)| body.windows(needle.len()).any(|w| w == needle.as_bytes()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_otlp_configuration_starts_export_once() {
    let (url, received) = collector().await;
    let mut guard = TelemetryBuilder::new("late-otlp")
        .without_env_var()
        .with_late_configuration()
        .init()
        .expect("init");

    tracing::info!("emitted before the attach");
    guard
        .apply_late_configuration(LateConfiguration::new().with_otlp_config(OtlpConfig {
            url,
            ..OtlpConfig::default()
        }))
        .expect("attach");
    tracing::info!("emitted after the attach");

    assert!(matches!(
        guard.apply_late_configuration(LateConfiguration::new()),
        Err(TelemetryError::LateConfigurationUnavailable)
    ));

    guard.shutdown().await.expect("shutdown flushes");
    assert!(log_bodies_contain(&received, "emitted after the attach"));
    assert!(!log_bodies_contain(&received, "emitted before the attach"));
}
