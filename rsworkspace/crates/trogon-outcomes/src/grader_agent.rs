use bytes::Bytes;
use serde::{Deserialize, Serialize};
use trogon_orchestrator::caller::AgentCaller;
use trogon_registry::AgentCapability;
use trogon_transcript::TranscriptEntry;

use crate::{
    provider::EvaluationProvider,
    types::{CriterionScore, OutcomesError, Rubric},
};

/// Wire format sent to a grader sub-agent.
///
/// The grader receives the full rubric definition and the session transcript
/// so it can evaluate the output independently without any shared context
/// with the agent being graded.
#[derive(Debug, Serialize)]
pub struct GraderRequest<'a> {
    pub rubric: &'a Rubric,
    pub transcript: &'a [TranscriptEntry],
}

/// Expected reply from a grader sub-agent.
#[derive(Debug, Deserialize)]
pub struct GraderResponse {
    pub scores: Vec<CriterionScore>,
    pub reasoning: String,
}

/// An `EvaluationProvider` that dispatches to a registered grader sub-agent
/// via NATS rather than calling an LLM directly.
///
/// This is the "isolated verifier" pattern: the grader runs as an independent
/// process with its own context and cannot be influenced by the agent being
/// evaluated. The `RalphLoop` uses this as the evaluation step when strict
/// isolation is required.
#[derive(Clone)]
pub struct SubAgentEvaluationProvider<C: AgentCaller> {
    caller: C,
    capability: AgentCapability,
}

impl<C: AgentCaller> SubAgentEvaluationProvider<C> {
    pub fn new(caller: C, capability: AgentCapability) -> Self {
        Self { caller, capability }
    }
}

impl<C: AgentCaller> EvaluationProvider for SubAgentEvaluationProvider<C> {
    async fn evaluate(
        &self,
        rubric: &Rubric,
        transcript: &[TranscriptEntry],
    ) -> Result<(Vec<CriterionScore>, String), OutcomesError> {
        let request = GraderRequest { rubric, transcript };
        let payload = serde_json::to_vec(&request)
            .map(Bytes::from)
            .map_err(|e| OutcomesError::Parse(e.to_string()))?;

        let response_bytes = self
            .caller
            .call(&self.capability, payload)
            .await
            .map_err(|e| OutcomesError::Llm(e.to_string()))?;

        let response: GraderResponse = serde_json::from_slice(&response_bytes)
            .map_err(|e| OutcomesError::Parse(format!("grader response: {e}")))?;

        Ok((response.scores, response.reasoning))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::future::Future;

    use super::*;
    use crate::types::Criterion;
    use trogon_orchestrator::caller::mock::MockAgentCaller;
    use trogon_transcript::entry::Role;

    fn cap() -> AgentCapability {
        AgentCapability::new("GraderAgent", ["grade"], "agents.grader.>")
    }

    fn transcript() -> Vec<TranscriptEntry> {
        let now = trogon_transcript::entry::now_ms();
        vec![
            TranscriptEntry::Message {
                role: Role::User,
                content: "Write a summary.".into(),
                timestamp: now,
                tokens: None,
            },
            TranscriptEntry::Message {
                role: Role::Assistant,
                content: "Here is the summary.".into(),
                timestamp: now,
                tokens: None,
            },
        ]
    }

    fn rubric() -> Rubric {
        Rubric::new(
            "r1",
            "Quality",
            "Measures output quality",
            vec![Criterion {
                name: "clarity".into(),
                description: "Is it clear?".into(),
                weight: 1.0,
            }],
        )
    }

    fn grader_response(score: f32) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "scores": [{"criterion": "clarity", "score": score, "reasoning": "looks good"}],
            "reasoning": "overall fine"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn dispatches_to_grader_and_returns_scores() {
        let caller = MockAgentCaller::returning(grader_response(0.9));
        let provider = SubAgentEvaluationProvider::new(caller, cap());

        let (scores, reasoning) = provider.evaluate(&rubric(), &transcript()).await.unwrap();

        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].criterion, "clarity");
        assert!((scores[0].score - 0.9).abs() < 1e-5);
        assert_eq!(reasoning, "overall fine");
    }

    #[tokio::test]
    async fn caller_error_maps_to_llm_error() {
        let caller = MockAgentCaller::failing("grader agent unavailable");
        let provider = SubAgentEvaluationProvider::new(caller, cap());

        let err = provider.evaluate(&rubric(), &transcript()).await.unwrap_err();

        assert!(
            matches!(err, OutcomesError::Llm(_)),
            "caller failure should be OutcomesError::Llm"
        );
        assert!(err.to_string().contains("grader agent unavailable"));
    }

    #[tokio::test]
    async fn invalid_grader_response_maps_to_parse_error() {
        let caller = MockAgentCaller::returning(b"not valid json".to_vec());
        let provider = SubAgentEvaluationProvider::new(caller, cap());

        let err = provider.evaluate(&rubric(), &transcript()).await.unwrap_err();

        assert!(
            matches!(err, OutcomesError::Parse(_)),
            "bad JSON response should be OutcomesError::Parse"
        );
    }

    #[tokio::test]
    async fn request_payload_includes_rubric_and_transcript() {
        // Capture the raw payload sent to the caller and verify it contains
        // the rubric id and transcript content.
        #[derive(Clone)]
        struct CapturingCaller {
            captured: std::sync::Arc<std::sync::Mutex<Option<Vec<u8>>>>,
        }

        impl AgentCaller for CapturingCaller {
            fn call<'a>(
                &'a self,
                _cap: &'a AgentCapability,
                payload: Bytes,
            ) -> impl Future<
                Output = Result<Bytes, trogon_orchestrator::types::OrchestratorError>,
            > + Send + 'a {
                *self.captured.lock().unwrap() = Some(payload.to_vec());
                let response = grader_response(0.8);
                async move { Ok(Bytes::from(response)) }
            }
        }

        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let caller = CapturingCaller { captured: captured.clone() };
        let provider = SubAgentEvaluationProvider::new(caller, cap());

        provider.evaluate(&rubric(), &transcript()).await.unwrap();

        let payload = captured.lock().unwrap().clone().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["rubric"]["id"], "r1");
        assert!(json["transcript"].as_array().unwrap().len() == 2);
    }

    #[tokio::test]
    async fn provider_is_clone() {
        let caller = MockAgentCaller::returning(grader_response(1.0));
        let provider = SubAgentEvaluationProvider::new(caller, cap());
        let _clone = provider.clone();
    }
}
