// SPDX-License-Identifier: MIT

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use telemetry_setup::{LogControlConfig, OtlpConfig, TelemetryBuilder};
use tracing::Level;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("read local addr").port();
    drop(listener);
    port
}

fn unused_local_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("read local addr");
    drop(listener);
    format!("http://{addr}")
}

async fn raw_http_request(port: u16, request: String) -> String {
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect log-control");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        stream.write_all(request.as_bytes()).expect("write request");
        stream.flush().expect("flush request");

        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    })
    .await
    .expect("join blocking request")
}

async fn raw_http_get(port: u16, path: &str) -> String {
    raw_http_request(
        port,
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    )
    .await
}

async fn raw_http_put_json(port: u16, path: &str, body: &str) -> String {
    raw_http_request(
        port,
        format!(
            "PUT {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

#[tokio::test]
async fn otlp_filter_update_reloads_live_filter_behavior() {
    let port = free_port();
    let mut guard = TelemetryBuilder::new("test-otlp-log-control")
        .without_env_var()
        .with_stdout_filter("error")
        .with_otlp_config(OtlpConfig {
            url: unused_local_url(),
            log_level: "warn".to_string(),
            ..OtlpConfig::default()
        })
        .with_log_control(LogControlConfig { port })
        .init()
        .expect("init with otlp and log-control should succeed");

    assert!(!tracing::enabled!(target: "telemetry_setup_otlp_reload_probe", Level::INFO));

    let put_response = raw_http_put_json(
        port,
        "/filters/otlp",
        r#"{"filter":"warn,telemetry_setup_otlp_reload_probe=info"}"#,
    )
    .await;
    assert!(put_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(put_response.contains("\"stdout\":\"error\""));
    assert!(put_response.contains("\"otlp\":\"warn,telemetry_setup_otlp_reload_probe=info\""));

    let get_response = raw_http_get(port, "/filters").await;
    assert!(get_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(get_response.contains("\"stdout\":\"error\""));
    assert!(get_response.contains("\"otlp\":\"warn,telemetry_setup_otlp_reload_probe=info\""));

    assert!(tracing::enabled!(target: "telemetry_setup_otlp_reload_probe", Level::INFO));

    guard.shutdown().await.expect("shutdown should succeed");
}
