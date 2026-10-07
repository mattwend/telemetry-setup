// SPDX-License-Identifier: MIT

// `init()` installs a process-global subscriber, so this case needs its own
// test binary.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opentelemetry::global;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::metrics::v1::{metric, number_data_point};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
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

/// Decodes the OTLP requests in `received` sent to `path`.
///
/// Returns the decoded messages, panicking if any matching body is invalid.
fn exported_requests<T: Message + Default>(received: &Received, path: &str) -> Vec<T> {
    received
        .lock()
        .expect("sink")
        .iter()
        .filter(|(request_path, _)| request_path == path)
        .map(|(_, body)| T::decode(body.as_slice()).expect("decode OTLP request"))
        .collect()
}

/// Asserts that `resource` carries `expected` as its `service.name` attribute.
///
/// Returns nothing, panicking if the resource or the expected attribute is absent.
fn assert_service_name(resource: Option<&Resource>, expected: &str) {
    let resource = resource.expect("exported resource");
    let service_name = resource
        .attributes
        .iter()
        .find(|attribute| attribute.key == "service.name")
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| value.value.as_ref());
    assert_eq!(
        service_name,
        Some(&Value::StringValue(expected.to_string()))
    );
}

/// Verifies late export of all signals, their resources, and one-shot attachment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_otlp_configuration_starts_export_once() {
    let (url, received) = collector().await;
    let service_name = "late-otlp-override";
    let mut guard = TelemetryBuilder::new("late-otlp")
        .without_env_var()
        .with_stdout_filter("off")
        .with_late_configuration()
        .init()
        .expect("init");

    tracing::info_span!("before_attach").in_scope(|| {
        tracing::info!("emitted before the attach");
    });
    global::meter("late-otlp-test")
        .u64_counter("late_otlp_before_attach")
        .build()
        .add(1, &[]);

    guard
        .apply_late_configuration(LateConfiguration::new().with_otlp_config(OtlpConfig {
            url,
            service_name: Some(service_name.to_string()),
            // Keep periodic export out of the test; shutdown must flush all signals.
            metrics_interval: Duration::from_secs(86400),
            ..OtlpConfig::default()
        }))
        .expect("attach");
    tracing::info_span!("after_attach").in_scope(|| {
        tracing::info!("emitted after the attach");
    });
    // Obtain a fresh meter after attach: existing meters keep their old provider.
    global::meter("late-otlp-test")
        .u64_counter("late_otlp_after_attach")
        .build()
        .add(7, &[]);

    assert!(matches!(
        guard.apply_late_configuration(LateConfiguration::new()),
        Err(TelemetryError::LateConfigurationUnavailable)
    ));

    guard.shutdown().await.expect("shutdown flushes");

    let mut log_bodies = Vec::new();
    for request in exported_requests::<ExportLogsServiceRequest>(&received, "/v1/logs") {
        for logs in request.resource_logs {
            assert_service_name(logs.resource.as_ref(), service_name);
            log_bodies.extend(
                logs.scope_logs
                    .into_iter()
                    .flat_map(|scope| scope.log_records)
                    .filter_map(|record| record.body.and_then(|body| body.value)),
            );
        }
    }
    assert!(log_bodies.contains(&Value::StringValue("emitted after the attach".to_string())));
    assert!(!log_bodies.contains(&Value::StringValue("emitted before the attach".to_string())));

    let mut span_names = Vec::new();
    for request in exported_requests::<ExportTraceServiceRequest>(&received, "/v1/traces") {
        for traces in request.resource_spans {
            assert_service_name(traces.resource.as_ref(), service_name);
            span_names.extend(
                traces
                    .scope_spans
                    .into_iter()
                    .flat_map(|scope| scope.spans)
                    .map(|span| span.name),
            );
        }
    }
    assert!(span_names.iter().any(|name| name == "after_attach"));
    assert!(!span_names.iter().any(|name| name == "before_attach"));

    let mut metrics = Vec::new();
    for request in exported_requests::<ExportMetricsServiceRequest>(&received, "/v1/metrics") {
        for resource_metrics in request.resource_metrics {
            assert_service_name(resource_metrics.resource.as_ref(), service_name);
            metrics.extend(
                resource_metrics
                    .scope_metrics
                    .into_iter()
                    .flat_map(|scope| scope.metrics),
            );
        }
    }
    assert!(
        metrics
            .iter()
            .all(|metric| metric.name != "late_otlp_before_attach")
    );
    let counter = metrics
        .iter()
        .find(|metric| metric.name == "late_otlp_after_attach")
        .expect("counter exported after attach");
    let Some(metric::Data::Sum(sum)) = &counter.data else {
        panic!("counter must export a sum: {:?}", counter.data);
    };
    assert!(sum.is_monotonic);
    assert!(
        sum.data_points
            .iter()
            .any(|point| point.value == Some(number_data_point::Value::AsInt(7)))
    );
}
