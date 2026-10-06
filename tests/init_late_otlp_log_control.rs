// SPDX-License-Identifier: MIT

// `init()` installs a process-global subscriber, so this case needs its own
// test binary.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use telemetry_setup::{LateConfiguration, LogControlConfig, OtlpConfig, TelemetryBuilder};
use tracing::Level;

fn reserve_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("read local addr").port()
}

fn unused_local_url() -> String {
    format!("http://127.0.0.1:{}", reserve_port())
}

async fn raw_http_request(port: u16, request: String) -> String {
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect log-control");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        stream.write_all(request.as_bytes()).expect("write request");

        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    })
    .await
    .expect("join blocking request")
}

async fn get_filters(port: u16) -> String {
    raw_http_request(
        port,
        "GET /filters HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n".to_string(),
    )
    .await
}

async fn put_otlp_filter(port: u16, filter: &str) -> String {
    let body = format!(r#"{{"filter":"{filter}"}}"#);
    raw_http_request(
        port,
        format!(
            "PUT /filters/otlp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn log_control_manages_a_late_otlp_filter() {
    let port = reserve_port();
    let mut guard = TelemetryBuilder::new("late-otlp-log-control")
        .without_env_var()
        .with_stdout_filter("error")
        .with_late_configuration()
        .with_log_control(LogControlConfig { port })
        .init()
        .expect("init");

    assert!(get_filters(port).await.contains("\"otlp\":null"));
    assert!(
        put_otlp_filter(port, "debug")
            .await
            .starts_with("HTTP/1.1 404 Not Found\r\n")
    );

    guard
        .apply_late_configuration(
            LateConfiguration::new()
                .with_stdout_filter("off")
                .with_otlp_config(OtlpConfig {
                    url: unused_local_url(),
                    log_level: "warn".to_string(),
                    ..OtlpConfig::default()
                }),
        )
        .expect("apply");

    let filters = get_filters(port).await;
    assert!(filters.contains("\"stdout\":\"off\""));
    assert!(filters.contains("\"otlp\":\"warn\""));
    assert!(!tracing::enabled!(target: "late_otlp_probe", Level::INFO));

    let updated = put_otlp_filter(port, "warn,late_otlp_probe=info").await;
    assert!(updated.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(updated.contains("\"otlp\":\"warn,late_otlp_probe=info\""));
    assert!(tracing::enabled!(target: "late_otlp_probe", Level::INFO));

    guard.shutdown().await.expect("shutdown");
}
