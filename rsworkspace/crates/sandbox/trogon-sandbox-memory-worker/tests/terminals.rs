mod common;
mod nats_common;

use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use common::{TestResult, start};
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, TerminalError, TerminalOutput, TerminalResponse};
use trogon_sandbox_memory_worker::{FrameSink, TerminalHub};
use trogon_sandbox_protocol::MAX_TERMINALS_PER_LEASE;

#[derive(Default)]
struct Collect(Mutex<Vec<(String, Bytes)>>);

#[async_trait]
impl FrameSink for Collect {
    async fn publish(&self, subject: String, payload: Bytes) {
        if let Ok(mut frames) = self.0.lock() {
            frames.push((subject, payload));
        }
    }
}

impl Collect {
    fn frames(&self) -> Result<Vec<TerminalOutput>, Box<dyn std::error::Error>> {
        let frames = self.0.lock().map_err(|e| e.to_string())?;
        frames
            .iter()
            .map(|(_, bytes)| Ok(serde_json::from_slice(bytes)?))
            .collect()
    }
}

fn open(lease: &str, terminal: &str) -> Vec<u8> {
    request(lease, terminal, json!({"op": "open", "cols": 80, "rows": 24}))
}

fn request(lease: &str, terminal: &str, op: Value) -> Vec<u8> {
    let mut body = json!({"lease_id": lease, "terminal_id": terminal});
    if let (Some(map), Some(op)) = (body.as_object_mut(), op.as_object()) {
        map.extend(op.clone());
    }
    body.to_string().into_bytes()
}

#[tokio::test]
async fn the_hub_echoes_and_enforces_ownership() -> TestResult {
    let hub = TerminalHub::default();
    let sink = Collect::default();
    let (l1, l2) = (LeaseId::new("l1"), LeaseId::new("l2"));

    assert!(matches!(
        hub.handle(&sink, &l1, b"{").await,
        TerminalResponse::Failed(TerminalError::InvalidRequest { .. })
    ));
    assert!(matches!(
        hub.handle(&sink, &l1, &open("other", "t1")).await,
        TerminalResponse::Failed(TerminalError::InvalidRequest { .. })
    ));
    assert!(matches!(
        hub.handle(&sink, &l1, &open("l1", "t1")).await,
        TerminalResponse::Opened { .. }
    ));
    assert!(matches!(
        hub.handle(&sink, &l1, &open("l1", "t1")).await,
        TerminalResponse::Opened { .. }
    ));
    assert!(matches!(
        hub.handle(&sink, &l2, &open("l2", "t1")).await,
        TerminalResponse::Failed(TerminalError::IdInUse)
    ));
    assert_eq!(hub.open_count(&l1), 1);

    let input = request("l1", "t1", json!({"op": "input", "data": "YQ0="}));
    assert!(matches!(hub.handle(&sink, &l1, &input).await, TerminalResponse::Done));
    let resize = request("l1", "t1", json!({"op": "resize", "cols": 100, "rows": 30}));
    assert!(matches!(hub.handle(&sink, &l1, &resize).await, TerminalResponse::Done));

    let stolen = request("l2", "t1", json!({"op": "input", "data": "YQ=="}));
    assert!(matches!(
        hub.handle(&sink, &l2, &stolen).await,
        TerminalResponse::Failed(TerminalError::IdInUse)
    ));
    let missing_input = request("l1", "gone", json!({"op": "input", "data": "YQ=="}));
    assert!(matches!(
        hub.handle(&sink, &l1, &missing_input).await,
        TerminalResponse::Failed(TerminalError::NotFound)
    ));
    let missing_resize = request("l1", "gone", json!({"op": "resize", "cols": 1, "rows": 1}));
    assert!(matches!(
        hub.handle(&sink, &l1, &missing_resize).await,
        TerminalResponse::Failed(TerminalError::NotFound)
    ));
    let stolen_close = request("l2", "t1", json!({"op": "close"}));
    assert!(matches!(
        hub.handle(&sink, &l2, &stolen_close).await,
        TerminalResponse::Failed(TerminalError::IdInUse)
    ));
    let unknown_close = request("l1", "gone", json!({"op": "close"}));
    assert!(matches!(
        hub.handle(&sink, &l1, &unknown_close).await,
        TerminalResponse::Done
    ));

    let close = request("l1", "t1", json!({"op": "close"}));
    assert!(matches!(hub.handle(&sink, &l1, &close).await, TerminalResponse::Done));
    assert_eq!(hub.open_count(&l1), 0);

    for n in 0..MAX_TERMINALS_PER_LEASE {
        assert!(matches!(
            hub.handle(&sink, &l1, &open("l1", &format!("m{n}"))).await,
            TerminalResponse::Opened { .. }
        ));
    }
    assert!(matches!(
        hub.handle(&sink, &l1, &open("l1", "overflow")).await,
        TerminalResponse::Failed(TerminalError::TooManyTerminals { .. })
    ));
    hub.close_lease(&sink, &l1).await;
    assert_eq!(hub.open_count(&l1), 0);

    let frames = sink.frames()?;
    assert!(matches!(frames.first(), Some(TerminalOutput::Data { seq: 0, .. })));
    assert!(frames.iter().any(|f| matches!(f, TerminalOutput::Exit { .. })));
    Ok(())
}

#[tokio::test]
async fn terminals_publish_their_frames_over_nats() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;
    let id = trogon_sandbox_contract::TerminalId::new("t1")?;
    let mut frames = h
        .client
        .subscribe(trogon_sandbox_protocol::subjects::terminal_output(&id))
        .await?;
    h.client.flush().await?;
    let reply = h
        .call_raw("terminal", "l1", serde_json::from_slice(&open("l1", "t1"))?)
        .await?;
    assert!(reply.to_string().contains("opened"));
    h.call_raw(
        "terminal",
        "l1",
        serde_json::from_slice(&request("l1", "t1", json!({"op": "input", "data": "aGk="})))?,
    )
    .await?;
    let mut seen = Vec::new();
    while seen.len() < 2 {
        let message = tokio::time::timeout(std::time::Duration::from_secs(10), frames.next())
            .await?
            .ok_or("closed")?;
        seen.push(serde_json::from_slice::<TerminalOutput>(&message.payload)?);
    }
    assert!(matches!(seen[0], TerminalOutput::Data { seq: 0, .. }));
    assert!(matches!(seen[1], TerminalOutput::Data { seq: 1, .. }));
    h.worker.shutdown().await;
    let message = tokio::time::timeout(std::time::Duration::from_secs(10), frames.next())
        .await?
        .ok_or("closed")?;
    assert!(matches!(
        serde_json::from_slice::<TerminalOutput>(&message.payload)?,
        TerminalOutput::Exit { .. }
    ));
    Ok(())
}
