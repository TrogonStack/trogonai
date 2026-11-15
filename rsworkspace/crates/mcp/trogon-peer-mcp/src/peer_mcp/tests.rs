use std::future::{Future as _, poll_fn};
use std::sync::Mutex;
use std::task::Poll;

use serde_json::json;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Default)]
struct Backend {
    enrollments: Mutex<Vec<v1::EnrollRequest>>,
    preparations: Mutex<Vec<v1::PrepareBatchRequest>>,
}

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error("initial-session fixture recorder is poisoned")]
    Poisoned,
    #[error("initial-session enrollment fixture is invalid")]
    Enrollment(#[from] serde_json::Error),
    #[error("operation is not configured in the initial-session fixture")]
    Unsupported,
}

impl EnrollPeer for Backend {
    type Error = FixtureError;

    async fn enroll(&self, request: v1::EnrollRequest) -> Result<v1::EnrollResponse, Self::Error> {
        self.enrollments
            .lock()
            .map_err(|_| FixtureError::Poisoned)?
            .push(request);
        let peer_id = "b".repeat(64);
        Ok(serde_json::from_value(json!({
            "peerId": peer_id,
            "attachment": {
                "attachmentRef": {"peerId": peer_id, "connectorId": "connector_b", "generation": 1},
                "status": "ATTACHMENT_STATUS_ATTACHED",
                "observedAt": "2026-11-02T12:00:00Z",
                "expiresAt": "2026-11-02T12:00:10Z"
            }
        }))?)
    }
}

impl PreparePeerBatch for Backend {
    type Error = FixtureError;

    async fn prepare_batch(&self, request: v1::PrepareBatchRequest) -> Result<v1::PrepareBatchResponse, Self::Error> {
        self.preparations
            .lock()
            .map_err(|_| FixtureError::Poisoned)?
            .push(request);
        Ok(Default::default())
    }
}

impl ReadPeerRoster for Backend {
    type Error = FixtureError;

    async fn roster(&self, _request: v1::RosterRequest) -> Result<v1::RosterResponse, Self::Error> {
        Err(FixtureError::Unsupported)
    }
}

impl SendPeerMessage for Backend {
    type Error = FixtureError;

    async fn send(&self, _request: v1::SendRequest) -> Result<v1::SendResponse, Self::Error> {
        Err(FixtureError::Unsupported)
    }
}

impl ReadPeerMessageStatus for Backend {
    type Error = FixtureError;

    async fn status(&self, _request: v1::StatusRequest) -> Result<v1::StatusResponse, Self::Error> {
        Err(FixtureError::Unsupported)
    }
}

impl AcknowledgePeerBatch for Backend {
    type Error = FixtureError;

    async fn acknowledge_batch(
        &self,
        _request: v1::AcknowledgeBatchRequest,
    ) -> Result<v1::AcknowledgeBatchResponse, Self::Error> {
        Err(FixtureError::Unsupported)
    }
}

impl HeartbeatPeer for Backend {
    type Error = FixtureError;

    async fn heartbeat(&self, _request: v1::HeartbeatRequest) -> Result<v1::HeartbeatResponse, Self::Error> {
        Err(FixtureError::Unsupported)
    }
}

impl DetachPeer for Backend {
    type Error = FixtureError;

    async fn detach(&self, _request: v1::DetachRequest) -> Result<v1::DetachResponse, Self::Error> {
        Err(FixtureError::Unsupported)
    }
}

fn prompt(session: &str, prompt: &str) -> Result<CallToolRequestParams, serde_json::Error> {
    serde_json::from_value(json!({
        "name": "peer_prompt_hook",
        "arguments": {"session_id": session, "prompt_id": prompt, "hook_event_name": "UserPromptSubmit"}
    }))
}

#[tokio::test]
async fn queued_initial_native_sessions_recheck_identity_before_backend_work() -> TestResult {
    let adapter = PeerMcp::new(
        Backend::default(),
        ConnectorConfigInput {
            installation_id: "installation_b".into(),
            connector_id: "connector_b".into(),
            collaboration_ref: "collaboration_a".into(),
            display_name: "reviewer".into(),
        },
    )?;
    let operation = adapter.operation.lock().await;
    let first = adapter.invoke(prompt("native_first", "first_prompt")?);
    let second = adapter.invoke(prompt("native_second", "second_prompt")?);
    tokio::pin!(first, second);
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    assert!(poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx).is_pending())).await);
    assert!(
        adapter.binding.get().is_none(),
        "both prompts must queue before a session binds"
    );
    assert!(
        adapter
            .backend
            .enrollments
            .lock()
            .map_err(|_| FixtureError::Poisoned)?
            .is_empty()
    );
    assert!(
        adapter
            .backend
            .preparations
            .lock()
            .map_err(|_| FixtureError::Poisoned)?
            .is_empty()
    );
    drop(operation);

    let (first, second) = tokio::join!(first, second);
    first?;
    assert!(matches!(second, Err(PeerMcpError::NativeSessionChanged)));
    {
        let enrollments = adapter.backend.enrollments.lock().map_err(|_| FixtureError::Poisoned)?;
        let preparations = adapter
            .backend
            .preparations
            .lock()
            .map_err(|_| FixtureError::Poisoned)?;
        assert_eq!(enrollments.len(), 1, "losing session must not enroll");
        assert_eq!(
            enrollments
                .first()
                .and_then(|request| request.native_session_id.as_deref()),
            Some("native_first")
        );
        assert_eq!(preparations.len(), 1, "losing session must not prepare");
        assert_eq!(
            preparations.first().and_then(|request| request.prompt_id.as_deref()),
            Some("first_prompt")
        );
    }
    assert!(matches!(
        adapter.invoke(prompt("native_second", "retry_losing_session")?).await,
        Err(PeerMcpError::NativeSessionChanged)
    ));
    adapter.invoke(prompt("native_first", "retry_bound_session")?).await?;
    assert_eq!(
        adapter
            .backend
            .enrollments
            .lock()
            .map_err(|_| FixtureError::Poisoned)?
            .len(),
        2
    );
    assert_eq!(
        adapter
            .backend
            .preparations
            .lock()
            .map_err(|_| FixtureError::Poisoned)?
            .len(),
        2
    );
    Ok(())
}
