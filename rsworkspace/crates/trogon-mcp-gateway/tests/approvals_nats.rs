//! Approval awaiter resolves when a decision is published on `mcp.approvals.{request_id}`.

use std::time::Duration;

use trogon_mcp_gateway::approvals::{
    ApprovalCache, ApprovalClient, ApprovalDecisionKind, ApprovalDecisionMessage, ApprovalSubject,
    ApprovalWaitOutcome, ArgsHash, RequestId,
};
use trogon_nats::{NatsAuth, NatsConfig, connect};

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway -- --ignored"]
async fn approval_decision_resolves_awaiter() {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);
    let client = connect(&nats_conf, Duration::from_secs(15))
        .await
        .expect("connect nats");

    let request_id = RequestId::new(format!("req-{}", uuid::Uuid::now_v7())).expect("request id");
    let subject = ApprovalSubject::for_request(&request_id);
    let args_hash = ArgsHash::from_json(&serde_json::json!({"tool": "deploy"}));
    let cache = ApprovalCache::new(100);
    let approval_client = ApprovalClient::new(client.clone(), cache.clone());

    let waiter_subject = subject.clone();
    let waiter_request_id = request_id.clone();
    let waiter_args = args_hash.clone();
    let waiter = tokio::spawn(async move {
        approval_client
            .await_decision(
                &waiter_subject,
                &waiter_request_id,
                &waiter_args,
                Duration::from_secs(5),
            )
            .await
            .expect("await decision")
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    let decision = ApprovalDecisionMessage {
        decision: ApprovalDecisionKind::Approve,
        approver: "human:alice".into(),
        expires_at: trogon_mcp_gateway::anomaly::now_unix() + 300,
    };
    client
        .publish(
            subject.as_str().to_string(),
            serde_json::to_vec(&decision).expect("serialize decision").into(),
        )
        .await
        .expect("publish approval");

    let outcome = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("waiter timed out")
        .expect("waiter join");
    assert!(matches!(outcome, ApprovalWaitOutcome::Approved { .. }));

    let cache_only = ApprovalClient::new(client, cache);
    let cached_outcome = cache_only
        .await_decision(&subject, &request_id, &args_hash, Duration::from_secs(1))
        .await
        .expect("cached approval");
    assert!(matches!(cached_outcome, ApprovalWaitOutcome::Approved { .. }));
}
