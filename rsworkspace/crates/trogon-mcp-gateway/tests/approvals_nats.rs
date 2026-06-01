//! Approval coordinator resolves when a decision is published on `mcp.approvals.{request_id}`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use trogon_mcp_gateway::approvals::{
    ApprovalAuditSink, ApprovalCoordinator, ApprovalDecision, ApprovalDecisionKind,
    ApprovalDecisionMessage, ApprovalError, ApprovalGate, ApprovalNatsListener, ApprovalRequest,
    ApprovalSubject, CoordinatorApprovalGate, RequestId,
};
use trogon_nats::{NatsAuth, NatsConfig, connect};

struct RecordingAudit {
    events: Arc<Mutex<Vec<String>>>,
}

impl RecordingAudit {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn events_arc(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.events)
    }

    fn snapshot(events: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        events.lock().expect("audit mutex poisoned").clone()
    }
}

impl ApprovalAuditSink for RecordingAudit {
    fn approval_requested(&self, request_id: &RequestId) {
        self.events
            .lock()
            .expect("audit mutex poisoned")
            .push(format!("approval.requested:{}", request_id.as_str()));
    }

    fn approval_granted(&self, request_id: &RequestId, approver: &str) {
        self.events.lock().expect("audit mutex poisoned").push(format!(
            "approval.granted:{}:{}",
            request_id.as_str(),
            approver
        ));
    }

    fn approval_denied(&self, request_id: &RequestId, approver: &str) {
        self.events.lock().expect("audit mutex poisoned").push(format!(
            "approval.denied:{}:{}",
            request_id.as_str(),
            approver
        ));
    }

    fn approval_expired(&self, request_id: &RequestId) {
        self.events
            .lock()
            .expect("audit mutex poisoned")
            .push(format!("approval.expired:{}", request_id.as_str()));
    }
}

struct ApprovalHarness {
    client: async_nats::Client,
    coordinator: Arc<ApprovalCoordinator>,
    _listener: tokio::task::JoinHandle<()>,
}

impl ApprovalHarness {
    async fn connect() -> Self {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);
        let client = connect(&nats_conf, Duration::from_secs(15))
            .await
            .expect("connect nats");
        let coordinator = Arc::new(ApprovalCoordinator::new());
        let listener = ApprovalNatsListener::start(
            client.clone(),
            "mcp.approvals",
            Arc::clone(&coordinator),
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        Self {
            client,
            coordinator,
            _listener: listener,
        }
    }

    fn gate(&self) -> CoordinatorApprovalGate {
        CoordinatorApprovalGate::new(Arc::clone(&self.coordinator))
    }
}

async fn request_with_audit(
    gate: &CoordinatorApprovalGate,
    audit: &RecordingAudit,
    request: &ApprovalRequest,
) -> Result<ApprovalDecision, ApprovalError> {
    audit.approval_requested(&request.request_id);
    match gate.request_approval(request).await {
        Ok(ApprovalDecision::Granted { approver, .. }) => {
            audit.approval_granted(&request.request_id, &approver);
            Ok(ApprovalDecision::Granted {
                approver,
                expires_at: trogon_mcp_gateway::anomaly::now_unix() + 300,
            })
        }
        Ok(ApprovalDecision::Denied { approver, reason }) => {
            audit.approval_denied(&request.request_id, &approver);
            Ok(ApprovalDecision::Denied { approver, reason })
        }
        Err(ApprovalError::Timeout) => {
            audit.approval_expired(&request.request_id);
            Err(ApprovalError::Timeout)
        }
        Err(error) => Err(error),
    }
}

#[tokio::test]
async fn approval_decision_resolves_awaiter() {
    let harness = ApprovalHarness::connect().await;
    let request_id = RequestId::new(format!("req-{}", uuid::Uuid::now_v7())).expect("request id");
    let subject = ApprovalSubject::for_request("mcp", &request_id);
    let request = ApprovalRequest::new("mcp", request_id.clone(), Duration::from_secs(5), None);
    let audit = RecordingAudit::new();
    let gate = harness.gate();
    let audit_events = audit.events_arc();

    let waiter = tokio::spawn(async move {
        request_with_audit(&gate, &audit, &request).await
    });

    tokio::time::sleep(Duration::from_millis(250)).await;
    let decision = ApprovalDecisionMessage {
        decision: ApprovalDecisionKind::Approve,
        approver: "human:alice".into(),
        expires_at: trogon_mcp_gateway::anomaly::now_unix() + 300,
    };
    harness
        .client
        .publish(
            subject.as_str().to_string(),
            serde_json::to_vec(&decision).expect("serialize decision").into(),
        )
        .await
        .expect("publish approval");

    let outcome = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("waiter timed out")
        .expect("waiter join")
        .expect("approved");
    assert!(matches!(outcome, ApprovalDecision::Granted { .. }));
    assert!(RecordingAudit::snapshot(&audit_events)
        .iter()
        .any(|event| event.starts_with("approval.granted:")));
}

#[tokio::test]
async fn approval_decision_denies_awaiter() {
    let harness = ApprovalHarness::connect().await;
    let request_id = RequestId::new(format!("req-deny-{}", uuid::Uuid::now_v7())).expect("request id");
    let subject = ApprovalSubject::for_request("mcp", &request_id);
    let request = ApprovalRequest::new("mcp", request_id, Duration::from_secs(5), None);
    let audit = RecordingAudit::new();
    let gate = harness.gate();
    let audit_events = audit.events_arc();

    let waiter = tokio::spawn(async move {
        request_with_audit(&gate, &audit, &request).await
    });

    tokio::time::sleep(Duration::from_millis(250)).await;
    let decision = ApprovalDecisionMessage {
        decision: ApprovalDecisionKind::Deny,
        approver: "human:bob".into(),
        expires_at: trogon_mcp_gateway::anomaly::now_unix() + 300,
    };
    harness
        .client
        .publish(
            subject.as_str().to_string(),
            serde_json::to_vec(&decision).expect("serialize decision").into(),
        )
        .await
        .expect("publish denial");

    let outcome = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("waiter timed out")
        .expect("waiter join")
        .expect("denied");
    assert!(matches!(outcome, ApprovalDecision::Denied { .. }));
    assert!(RecordingAudit::snapshot(&audit_events)
        .iter()
        .any(|event| event.starts_with("approval.denied:")));
}

#[tokio::test]
async fn approval_ttl_expiry_emits_expired_audit() {
    let harness = ApprovalHarness::connect().await;
    let request_id =
        RequestId::new(format!("req-expire-{}", uuid::Uuid::now_v7())).expect("request id");
    let request = ApprovalRequest::new("mcp", request_id.clone(), Duration::from_millis(150), None);
    let audit = RecordingAudit::new();
    let gate = harness.gate();

    let audit_events = audit.events_arc();

    let outcome = request_with_audit(&gate, &audit, &request).await;
    assert_eq!(outcome, Err(ApprovalError::Timeout));
    assert!(RecordingAudit::snapshot(&audit_events)
        .iter()
        .any(|event| event == &format!("approval.expired:{}", request_id.as_str())));
}

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway -- --ignored"]
async fn legacy_approval_client_cache_hit() {
    use trogon_mcp_gateway::approvals::{
        ApprovalCache, ApprovalClient, ApprovalWaitOutcome, ArgsHash,
    };

    let harness = ApprovalHarness::connect().await;
    let request_id = RequestId::new(format!("req-cache-{}", uuid::Uuid::now_v7())).expect("request id");
    let subject = ApprovalSubject::for_request("mcp", &request_id);
    let args_hash = ArgsHash::from_json(&serde_json::json!({"tool": "deploy"}));
    let cache = ApprovalCache::new(100);
    let approval_client = ApprovalClient::new(harness.client.clone(), cache.clone());

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
    harness
        .client
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

    let cache_only = ApprovalClient::new(harness.client, cache);
    let cached_outcome = cache_only
        .await_decision(&subject, &request_id, &args_hash, Duration::from_secs(1))
        .await
        .expect("cached approval");
    assert!(matches!(cached_outcome, ApprovalWaitOutcome::Approved { .. }));
}
