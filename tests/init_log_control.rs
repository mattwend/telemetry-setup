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

async fn raw_http_get(port: u16, path: &str) -> String {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect log-control");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .expect("write request");
        stream.flush().expect("flush request");

        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    })
    .await
    .expect("join blocking GET")
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

    let response = raw_http_get(port, "/filters").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\"stdout\":\"info\""));

    guard.shutdown().await.expect("shutdown should succeed");
}
