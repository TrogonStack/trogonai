//! The echo terminal hub driven directly through a collecting frame sink.

use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, TerminalError, TerminalOutput, TerminalResponse};
use trogon_sandbox_memory_worker::{FrameSink, TerminalHub};
use trogon_sandbox_protocol::{MAX_TERMINALS_PER_LEASE, terminal_output_bytes};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
    fn subjects(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let frames = self.0.lock().map_err(|e| e.to_string())?;
        Ok(frames.iter().map(|(subject, _)| subject.clone()).collect())
    }

    fn frames(&self) -> Result<Vec<TerminalOutput>, Box<dyn std::error::Error>> {
        let frames = self.0.lock().map_err(|e| e.to_string())?;
        frames
            .iter()
            .map(|(_, bytes)| Ok(serde_json::from_slice(bytes)?))
            .collect()
    }

    fn exits(&self) -> Result<usize, Box<dyn std::error::Error>> {
        Ok(self
            .frames()?
            .iter()
            .filter(|frame| matches!(frame, TerminalOutput::Exit { .. }))
            .count())
    }
}

fn request(lease: &str, terminal: &str, op: Value) -> Vec<u8> {
    let mut body = json!({"lease_id": lease, "terminal_id": terminal});
    if let (Some(map), Some(op)) = (body.as_object_mut(), op.as_object()) {
        map.extend(op.clone());
    }
    body.to_string().into_bytes()
}

fn open(lease: &str, terminal: &str) -> Vec<u8> {
    request(lease, terminal, json!({"op": "open", "cols": 80, "rows": 24}))
}

fn data_seq(frame: &TerminalOutput) -> Option<u64> {
    match frame {
        TerminalOutput::Data { seq, .. } => Some(*seq),
        TerminalOutput::Exit { .. } | TerminalOutput::Resync { .. } => None,
    }
}

#[tokio::test]
async fn open_prints_a_prompt_and_input_is_echoed_in_order() -> TestResult {
    let (hub, sink, lease) = (TerminalHub::default(), Collect::default(), LeaseId::new("L"));
    assert!(matches!(
        hub.handle(&sink, &lease, &open("L", "t1")).await,
        TerminalResponse::Opened { .. }
    ));
    // "bHMN" is the base64 of `ls` followed by a carriage return.
    let input = request("L", "t1", json!({"op": "input", "data": "bHMN"}));
    assert_eq!(hub.handle(&sink, &lease, &input).await, TerminalResponse::Done);

    let frames = sink.frames()?;
    assert_eq!(frames.len(), 2);
    assert_eq!(frames.first().and_then(terminal_output_bytes), Some(b"$ ".to_vec()));
    assert_eq!(frames.get(1).and_then(terminal_output_bytes), Some(b"ls\r\n".to_vec()));
    assert_eq!(frames.iter().map(data_seq).collect::<Vec<_>>(), vec![Some(0), Some(1)]);
    assert_eq!(
        sink.subjects()?.first().map(String::as_str),
        Some("sandbox.terminal.out.t1")
    );
    Ok(())
}

#[tokio::test]
async fn resize_is_observable_and_close_sends_one_exit_frame() -> TestResult {
    let (hub, sink, lease) = (TerminalHub::default(), Collect::default(), LeaseId::new("L"));
    hub.handle(&sink, &lease, &open("L", "t1")).await;
    let resize = request("L", "t1", json!({"op": "resize", "cols": 120, "rows": 40}));
    hub.handle(&sink, &lease, &resize).await;
    let close = request("L", "t1", json!({"op": "close"}));
    assert_eq!(hub.handle(&sink, &lease, &close).await, TerminalResponse::Done);

    let frames = sink.frames()?;
    assert_eq!(
        frames.get(1).and_then(terminal_output_bytes),
        Some(b"[resized 120x40]\r\n".to_vec())
    );
    assert!(matches!(
        frames.get(2),
        Some(TerminalOutput::Exit {
            seq: 2,
            exit_code: Some(0)
        })
    ));

    // A retried close is harmless and sends nothing more.
    assert_eq!(hub.handle(&sink, &lease, &close).await, TerminalResponse::Done);
    assert_eq!(sink.frames()?.len(), 3);
    let input = request("L", "t1", json!({"op": "input", "data": "eA=="}));
    assert_eq!(
        hub.handle(&sink, &lease, &input).await,
        TerminalResponse::Failed(TerminalError::NotFound)
    );
    Ok(())
}

#[tokio::test]
async fn another_lease_cannot_use_or_close_a_terminal() -> TestResult {
    let sink = Collect::default();
    let hub = TerminalHub::default();
    let (owner, other) = (LeaseId::new("A"), LeaseId::new("B"));
    hub.handle(&sink, &owner, &open("A", "t1")).await;
    let foreign = [
        request("B", "t1", json!({"op": "input", "data": "eA=="})),
        request("B", "t1", json!({"op": "close"})),
        open("B", "t1"),
    ];
    for body in foreign {
        assert_eq!(
            hub.handle(&sink, &other, &body).await,
            TerminalResponse::Failed(TerminalError::IdInUse)
        );
    }
    assert_eq!(hub.open_count(&owner), 1);
    assert_eq!(sink.frames()?.len(), 1, "no foreign output was produced");
    Ok(())
}

#[tokio::test]
async fn reopening_on_the_same_lease_sends_no_second_prompt() -> TestResult {
    let (hub, sink, lease) = (TerminalHub::default(), Collect::default(), LeaseId::new("L"));
    hub.handle(&sink, &lease, &open("L", "t1")).await;
    assert!(matches!(
        hub.handle(&sink, &lease, &open("L", "t1")).await,
        TerminalResponse::Opened { .. }
    ));
    assert_eq!(hub.open_count(&lease), 1);
    assert_eq!(sink.frames()?.len(), 1, "no second prompt");
    Ok(())
}

#[tokio::test]
async fn the_per_lease_cap_leaves_other_leases_alone() {
    let sink = Collect::default();
    let hub = TerminalHub::default();
    let (full, other) = (LeaseId::new("L"), LeaseId::new("M"));
    for n in 0..MAX_TERMINALS_PER_LEASE {
        assert!(matches!(
            hub.handle(&sink, &full, &open("L", &format!("t{n}"))).await,
            TerminalResponse::Opened { .. }
        ));
    }
    assert!(matches!(
        hub.handle(&sink, &full, &open("L", "extra")).await,
        TerminalResponse::Failed(TerminalError::TooManyTerminals { .. })
    ));
    assert!(matches!(
        hub.handle(&sink, &other, &open("M", "other")).await,
        TerminalResponse::Opened { .. }
    ));
}

#[tokio::test]
async fn releasing_a_lease_ends_only_its_own_terminals() -> TestResult {
    let sink = Collect::default();
    let hub = TerminalHub::default();
    let (released, kept) = (LeaseId::new("L"), LeaseId::new("M"));
    hub.handle(&sink, &released, &open("L", "t1")).await;
    hub.handle(&sink, &released, &open("L", "t2")).await;
    hub.handle(&sink, &kept, &open("M", "t3")).await;
    hub.close_lease(&sink, &released).await;
    assert_eq!(hub.open_count(&released), 0);
    assert_eq!(hub.open_count(&kept), 1);
    assert_eq!(sink.exits()?, 2);
    Ok(())
}

#[tokio::test]
async fn a_zero_sized_resize_is_an_invalid_request() {
    let (hub, sink, lease) = (TerminalHub::default(), Collect::default(), LeaseId::new("L"));
    let resize = request("L", "t1", json!({"op": "resize", "cols": 0, "rows": 1}));
    assert!(matches!(
        hub.handle(&sink, &lease, &resize).await,
        TerminalResponse::Failed(TerminalError::InvalidRequest { .. })
    ));
}
