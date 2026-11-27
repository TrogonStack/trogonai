mod common;

use bytes::Bytes;
use common::{MockBus, TestResult};
use futures::StreamExt;
use trogon_sandbox_client::{BusMessage, EvidenceFrame};
use trogon_sandbox_contract::LeaseId;

#[tokio::test]
async fn evidence_stream_yields_frames_in_order() -> TestResult {
    let lease_id = LeaseId::new("lease-stream");
    let messages = vec![
        BusMessage {
            subject: "sandbox.evidence.lease-stream.lifecycle".to_string(),
            payload: Bytes::from("frame-1"),
        },
        BusMessage {
            subject: "sandbox.evidence.lease-stream.review-result".to_string(),
            payload: Bytes::from("frame-2"),
        },
    ];
    let client = MockBus::with_subscribe_items(messages).into_client();
    let frames: Vec<_> = client.evidence_stream(&lease_id).await?.collect().await;
    let frames = frames.into_iter().collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        frames,
        vec![
            EvidenceFrame {
                subject_suffix: "lifecycle".to_string(),
                payload: Bytes::from("frame-1"),
            },
            EvidenceFrame {
                subject_suffix: "review-result".to_string(),
                payload: Bytes::from("frame-2"),
            },
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_subject_outside_the_lease_keeps_its_full_name() -> TestResult {
    let messages = vec![BusMessage {
        subject: "elsewhere.topic".to_string(),
        payload: Bytes::new(),
    }];
    let client = MockBus::with_subscribe_items(messages).into_client();
    let frames: Vec<_> = client
        .evidence_stream(&LeaseId::new("lease-stream"))
        .await?
        .collect()
        .await;
    let frame = frames.into_iter().next().ok_or("no frame")??;
    assert_eq!(frame.subject_suffix, "elsewhere.topic");
    Ok(())
}
