// SPDX-License-Identifier: MIT

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use telemetry_setup::{LogControlConfig, TelemetryBuilder};

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("read local addr").port();
    drop(listener);
    port
}

async fn raw_http_request(port: u16, request: String) -> String {
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect log-control");
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
async fn log_control_server_accepts_filter_update_after_init() {
    let port = free_port();
    let mut guard = TelemetryBuilder::new("test-lc")
        .without_env_var()
        .with_stdout_filter("info")
        .with_log_control(LogControlConfig { port })
        .init()
        .expect("init with log-control should succeed");

    let put_response = raw_http_put_json(port, "/filters/stdout", r#"{"filter":"debug"}"#).await;
    assert!(put_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(put_response.contains("\"stdout\":\"debug\""));

    let get_response = raw_http_get(port, "/filters").await;
    assert!(get_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(get_response.contains("\"stdout\":\"debug\""));

    guard.shutdown().await.expect("shutdown should succeed");
}
