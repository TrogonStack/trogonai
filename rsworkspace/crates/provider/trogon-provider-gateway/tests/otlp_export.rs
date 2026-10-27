#![cfg(unix)]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const PARENT_ID: &str = "00f067aa0ba902b7";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Signal {
    Traces,
    Metrics,
    Logs,
}

impl Signal {
    fn resource_key(self) -> &'static str {
        match self {
            Self::Traces => "resourceSpans",
            Self::Metrics => "resourceMetrics",
            Self::Logs => "resourceLogs",
        }
    }
}

struct Receipt {
    signal: Signal,
    body: Value,
}

async fn receive(
    State(sender): State<mpsc::UnboundedSender<Receipt>>,
    Path(signal): Path<Signal>,
    Json(body): Json<Value>,
) -> Json<Value> {
    sender.send(Receipt { signal, body }).unwrap();
    Json(json!({}))
}

async fn request(address: SocketAddr, path: &str) {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\ntraceparent: 00-{TRACE_ID}-{PARENT_ID}-01\r\ntracestate: vendor=opaque\r\nAuthorization: Bearer fixture-private-value\r\n\r\n"
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    let expected = if matches!(path, "/-/liveness" | "/-/readiness") {
        "200 OK"
    } else {
        "404 Not Found"
    };
    assert!(response.starts_with(&format!("HTTP/1.1 {expected}")), "{response}");
}

fn items<'a>(receipts: &'a [Receipt], signal: Signal, scope_key: &str, item_key: &str) -> Vec<&'a Value> {
    receipts
        .iter()
        .filter(|receipt| receipt.signal == signal)
        .flat_map(|receipt| receipt.body[signal.resource_key()].as_array().unwrap())
        .flat_map(|resource| resource[scope_key].as_array().unwrap())
        .flat_map(|scope| scope[item_key].as_array().unwrap())
        .collect()
}

fn attribute<'a>(attributes: &'a Value, key: &str) -> &'a Value {
    &attributes
        .as_array()
        .unwrap()
        .iter()
        .find(|attribute| attribute["key"] == key)
        .unwrap()["value"]
}

#[tokio::test]
async fn binary_exports_correlated_signals_and_flushes_on_sigterm() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (sender, mut receipts) = mpsc::unbounded_channel();
        let app = Router::new().route("/v1/{signal}", post(receive)).with_state(sender);
        let (stop_tx, stop_rx) = oneshot::channel();
        let receiver = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stop_rx.await;
                })
                .await
                .unwrap();
        });
        let logs = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_trogon-provider-gateway"))
            .args(["--host", "127.0.0.1", "--port", "0"])
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
            .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json")
            .env("OTEL_SERVICE_NAME", "provider-gateway-otel-test")
            .env(
                "OTEL_RESOURCE_ATTRIBUTES",
                "service.name=lower-priority,service.namespace=integration",
            )
            .env("OTEL_TRACES_SAMPLER", "parentbased_always_on")
            .env("OTEL_BSP_SCHEDULE_DELAY", "60000")
            .env("OTEL_BLRP_SCHEDULE_DELAY", "60000")
            .env("TROGON_LOG_DIR", logs.path())
            .env("RUST_LOG", "info")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (address_tx, address_rx) = oneshot::channel();
        let output = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut address_tx = Some(address_tx);
            let mut output = String::new();
            while let Some(line) = lines.next_line().await.unwrap() {
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && let Some(address) = value["fields"]["address"].as_str()
                    && let Ok(address) = address.parse::<SocketAddr>()
                    && let Some(sender) = address_tx.take()
                {
                    sender.send(address).unwrap();
                }
                output.push_str(&line);
                output.push('\n');
            }
            output
        });
        let address = address_rx.await.unwrap();
        for path in [
            "/-/liveness",
            "/-/readiness",
            "/-/unknown",
            "/not-implemented?key=fixture-private-value",
        ] {
            request(address, path).await;
        }
        assert!(
            Command::new("kill")
                .args(["-TERM", &child.id().unwrap().to_string()])
                .status()
                .await
                .unwrap()
                .success()
        );
        let status = child.wait().await.unwrap();
        let output = output.await.unwrap();
        assert!(status.success(), "{status}: {output}");
        stop_tx.send(()).unwrap();
        receiver.await.unwrap();
        receipts.close();
        let mut exports = Vec::new();
        while let Some(receipt) = receipts.recv().await {
            exports.push(receipt);
        }

        for signal in [Signal::Traces, Signal::Metrics, Signal::Logs] {
            let received: Vec<_> = exports.iter().filter(|receipt| receipt.signal == signal).collect();
            assert!(!received.is_empty(), "missing {signal:?}: {output}");
            for receipt in received {
                for resource in receipt.body[signal.resource_key()].as_array().unwrap() {
                    let attributes = &resource["resource"]["attributes"];
                    assert_eq!(
                        attribute(attributes, "service.name")["stringValue"],
                        "provider-gateway-otel-test"
                    );
                    assert_eq!(attribute(attributes, "service.namespace")["stringValue"], "integration");
                }
            }
        }
        let spans = items(&exports, Signal::Traces, "scopeSpans", "spans");
        assert_eq!(spans.len(), 1, "{spans:?}");
        let span = spans[0];
        assert_eq!(span["traceId"], TRACE_ID);
        assert_eq!(span["parentSpanId"], PARENT_ID);
        assert_eq!(span["traceState"], "vendor=opaque");
        assert_eq!(span["name"], "GET");
        assert_eq!(
            attribute(&span["attributes"], "http.request.method")["stringValue"],
            "GET"
        );
        assert_eq!(
            attribute(&span["attributes"], "http.response.status_code")["intValue"],
            "404"
        );
        let metrics = items(&exports, Signal::Metrics, "scopeMetrics", "metrics");
        let duration = metrics
            .iter()
            .find(|metric| metric["name"] == "http.server.request.duration")
            .unwrap();
        assert_eq!(duration["unit"], "s");
        let points = duration["histogram"]["dataPoints"].as_array().unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0]["count"], "1");
        assert_eq!(attribute(&points[0]["attributes"], "url.scheme")["stringValue"], "http");
        let records = items(&exports, Signal::Logs, "scopeLogs", "logRecords");
        assert!(
            records
                .iter()
                .any(|record| record["traceId"] == TRACE_ID && record["spanId"] == span["spanId"])
        );
        for receipt in &exports {
            let payload = receipt.body.to_string();
            assert!(
                !payload.contains("fixture-private-value"),
                "sensitive request data exported"
            );
            assert!(!payload.contains("/-/"), "operational request exported");
        }
    })
    .await
    .unwrap();
}
