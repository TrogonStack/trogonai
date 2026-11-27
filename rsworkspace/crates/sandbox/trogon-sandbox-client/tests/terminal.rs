mod common;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use common::{MockBus, TestResult, client_replying, lease_id};
use futures::StreamExt;
use trogon_sandbox_client::{BusMessage, ClientError};
use trogon_sandbox_contract::{
    Base64Data, LeaseError, TerminalId, TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
};

#[tokio::test]
async fn terminal_sends_the_request_on_the_lease_subject() -> TestResult {
    let response = TerminalResponse::Opened {
        terminal_id: TerminalId::new("t-1")?,
    };
    let (client, log) = client_replying(&response)?;
    let result = client
        .terminal(&lease_id(), TerminalId::new("t-1")?, TerminalOp::Close)
        .await?;
    assert_eq!(result, response);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.terminal.lease-1");
    assert_eq!(sent.timeout, std::time::Duration::from_secs(15));
    let request: TerminalRequest = serde_json::from_slice(&sent.payload)?;
    assert_eq!(request.terminal_id, TerminalId::new("t-1")?);
    assert_eq!(request.op, TerminalOp::Close);
    Ok(())
}

#[tokio::test]
async fn terminal_passes_failures_and_unsupported_through() -> TestResult {
    for response in [TerminalResponse::Unsupported, TerminalResponse::Done] {
        let (client, _log) = client_replying(&response)?;
        let result = client
            .terminal(&lease_id(), TerminalId::new("t-1")?, TerminalOp::Close)
            .await?;
        assert_eq!(result, response);
    }
    Ok(())
}

#[tokio::test]
async fn terminal_maps_denied_to_a_client_error() -> TestResult {
    let response = TerminalResponse::Denied(LeaseError::LeaseNotFound { lease_id: lease_id() });
    let (client, _log) = client_replying(&response)?;
    let result = client
        .terminal(&lease_id(), TerminalId::new("t-1")?, TerminalOp::Close)
        .await;
    assert!(matches!(result, Err(ClientError::Denied(_))));
    Ok(())
}

#[tokio::test]
async fn terminal_times_out_when_the_broker_is_silent() -> TestResult {
    let client = MockBus::timing_out().into_client();
    let result = client
        .terminal(&lease_id(), TerminalId::new("t-1")?, TerminalOp::Close)
        .await;
    assert!(matches!(result, Err(ClientError::Timeout(_))));
    Ok(())
}

#[tokio::test]
async fn terminal_output_decodes_frames_and_skips_garbage() -> TestResult {
    let subject = "sandbox.terminal.out.t-1".to_string();
    let frame = |output: &TerminalOutput| -> Result<BusMessage, serde_json::Error> {
        Ok(BusMessage {
            subject: subject.clone(),
            payload: Bytes::from(serde_json::to_vec(output)?),
        })
    };
    let prompt = TerminalOutput::Data {
        seq: 0,
        data: Base64Data::new(STANDARD.encode(b"$ "))?,
    };
    let exit = TerminalOutput::Exit {
        seq: 1,
        exit_code: Some(0),
    };
    let messages = vec![
        frame(&prompt)?,
        BusMessage {
            subject: subject.clone(),
            payload: Bytes::from("not json"),
        },
        frame(&exit)?,
    ];
    let client = MockBus::with_subscribe_items(messages).into_client();
    let frames: Vec<_> = client.terminal_output(&TerminalId::new("t-1")?).await?.collect().await;
    assert_eq!(frames, vec![prompt, exit]);
    Ok(())
}
