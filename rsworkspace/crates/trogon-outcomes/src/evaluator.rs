use trogon_transcript::TranscriptEntry;

use crate::{
    provider::EvaluationProvider,
    store::{ResultClient, RubricClient, OutcomesStore},
    types::{EvaluationResult, OutcomesError, Rubric},
};

/// Evaluates a session transcript against one or more rubrics.
///
/// Generic over the LLM provider (`P`) and the KV store (`S`) for full
/// unit-testability without a NATS server.
pub struct Evaluator<P: EvaluationProvider, S: OutcomesStore> {
    provider: P,
    rubrics: RubricClient<S>,
    results: ResultClient<S>,
}

impl<P: EvaluationProvider, S: OutcomesStore> Evaluator<P, S> {
    pub fn new(provider: P, rubric_store: S, result_store: S) -> Self {
        Self {
            provider,
            rubrics: RubricClient::new(rubric_store),
            results: ResultClient::new(result_store),
        }
    }

    /// Evaluate a completed session against specific rubrics (or all applicable ones).
    ///
    /// - If `rubric_ids` is non-empty, only those rubrics are applied.
    /// - Otherwise, all rubrics matching `actor_type` are applied.
    ///
    /// Stores each `EvaluationResult` in the results KV bucket.
    /// Returns the list of results produced.
    pub async fn evaluate_session(
        &self,
        actor_type: &str,
        actor_key: &str,
        session_id: &str,
        transcript: &[TranscriptEntry],
        rubric_ids: &[String],
    ) -> Result<Vec<EvaluationResult>, OutcomesError> {
        let rubrics = self.resolve_rubrics(actor_type, rubric_ids).await?;

        if rubrics.is_empty() {
            return Ok(vec![]);
        }

        let mut results = Vec::new();
        for rubric in &rubrics {
            match self.evaluate_one(actor_type, actor_key, session_id, transcript, rubric).await {
                Ok(result) => {
                    self.results.put(&result).await?;
                    results.push(result);
                }
                Err(e) => {
                    tracing::warn!(
                        rubric_id = %rubric.id,
                        error = %e,
                        "Evaluation failed for rubric, skipping"
                    );
                }
            }
        }

        Ok(results)
    }

    async fn evaluate_one(
        &self,
        actor_type: &str,
        actor_key: &str,
        session_id: &str,
        transcript: &[TranscriptEntry],
        rubric: &Rubric,
    ) -> Result<EvaluationResult, OutcomesError> {
        let (scores, reasoning) = self.provider.evaluate(rubric, transcript).await?;
        Ok(EvaluationResult::compute(
            rubric, session_id, actor_type, actor_key, scores, reasoning,
        ))
    }

    async fn resolve_rubrics(
        &self,
        actor_type: &str,
        rubric_ids: &[String],
    ) -> Result<Vec<Rubric>, OutcomesError> {
        if rubric_ids.is_empty() {
            // Apply all rubrics that match this actor type.
            let all = self.rubrics.list().await?;
            Ok(all.into_iter().filter(|r| r.matches_actor_type(actor_type)).collect())
        } else {
            let mut rubrics = Vec::new();
            for id in rubric_ids {
                if let Some(r) = self.rubrics.get(id).await? {
                    rubrics.push(r);
                } else {
                    tracing::warn!(rubric_id = %id, "Rubric not found, skipping");
                }
            }
            Ok(rubrics)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::store::mock::MockOutcomesStore;
    use crate::store::RubricClient;
    use crate::types::{Criterion, CriterionScore, now_ms};
    use trogon_transcript::entry::Role;

    // ── MockEvaluationProvider ────────────────────────────────────────────────

    #[derive(Clone)]
    struct MockEvaluationProvider {
        scores: Arc<Mutex<Vec<CriterionScore>>>,
        reasoning: Arc<Mutex<String>>,
        error: Arc<Mutex<Option<String>>>,
    }

    impl MockEvaluationProvider {
        fn returning(scores: Vec<CriterionScore>, reasoning: &str) -> Self {
            Self {
                scores: Arc::new(Mutex::new(scores)),
                reasoning: Arc::new(Mutex::new(reasoning.to_string())),
                error: Arc::new(Mutex::new(None)),
            }
        }

        fn failing(msg: &str) -> Self {
            Self {
                scores: Arc::new(Mutex::new(vec![])),
                reasoning: Arc::new(Mutex::new(String::new())),
                error: Arc::new(Mutex::new(Some(msg.to_string()))),
            }
        }
    }

    impl EvaluationProvider for MockEvaluationProvider {
        fn evaluate<'a>(
            &'a self,
            _rubric: &'a Rubric,
            _transcript: &'a [TranscriptEntry],
        ) -> impl Future<Output = Result<(Vec<CriterionScore>, String), OutcomesError>> + Send + 'a
        {
            let scores = self.scores.lock().unwrap().clone();
            let reasoning = self.reasoning.lock().unwrap().clone();
            let err = self.error.lock().unwrap().clone();
            async move {
                if let Some(e) = err {
                    Err(OutcomesError::Llm(e))
                } else {
                    Ok((scores, reasoning))
                }
            }
        }
    }

    fn rubric(id: &str, actor_filter: Option<&str>) -> Rubric {
        let mut r = Rubric::new(
            id,
            "Test Rubric",
            "desc",
            vec![Criterion { name: "quality".into(), description: "Is it good?".into(), weight: 1.0 }],
        );
        if let Some(f) = actor_filter {
            r = r.with_actor_type_filter(f);
        }
        r
    }

    fn good_scores() -> Vec<CriterionScore> {
        vec![CriterionScore { criterion: "quality".into(), score: 0.9, reasoning: "good".into() }]
    }

    fn sample_transcript() -> Vec<TranscriptEntry> {
        vec![TranscriptEntry::Message {
            role: Role::User,
            content: "Do a thing.".into(),
            timestamp: now_ms(),
            tokens: None,
        }]
    }

    async fn evaluator_with_rubric(
        rubric_id: &str,
        actor_filter: Option<&str>,
        provider: MockEvaluationProvider,
    ) -> Evaluator<MockEvaluationProvider, MockOutcomesStore> {
        let rubric_store = MockOutcomesStore::new();
        let result_store = MockOutcomesStore::new();
        let client = RubricClient::new(rubric_store.clone());
        client.put(&rubric(rubric_id, actor_filter)).await.unwrap();
        Evaluator::new(provider, rubric_store, result_store)
    }

    #[tokio::test]
    async fn evaluates_and_stores_result() {
        let provider = MockEvaluationProvider::returning(good_scores(), "great session");
        let ev = evaluator_with_rubric("r1", None, provider).await;

        let results = ev
            .evaluate_session("pr", "repo/1", "sess-1", &sample_transcript(), &[])
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].rubric_id, "r1");
        assert!(results[0].passed);
        assert!((results[0].overall_score - 0.9).abs() < 1e-5);
    }

    #[tokio::test]
    async fn evaluates_only_specified_rubrics() {
        let rubric_store = MockOutcomesStore::new();
        let result_store = MockOutcomesStore::new();
        let client = RubricClient::new(rubric_store.clone());
        client.put(&rubric("r1", None)).await.unwrap();
        client.put(&rubric("r2", None)).await.unwrap();

        let provider = MockEvaluationProvider::returning(good_scores(), "ok");
        let ev = Evaluator::new(provider, rubric_store, result_store);

        let results = ev
            .evaluate_session(
                "pr", "repo/1", "sess-1", &sample_transcript(),
                &["r1".to_string()],
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].rubric_id, "r1");
    }

    #[tokio::test]
    async fn skips_rubrics_that_dont_match_actor_type() {
        let provider = MockEvaluationProvider::returning(good_scores(), "ok");
        let ev = evaluator_with_rubric("r1", Some("issue"), provider).await;

        let results = ev
            .evaluate_session("pr", "repo/1", "sess-1", &sample_transcript(), &[])
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn skips_unknown_rubric_ids_gracefully() {
        let provider = MockEvaluationProvider::returning(good_scores(), "ok");
        let ev = evaluator_with_rubric("r1", None, provider).await;

        let results = ev
            .evaluate_session(
                "pr", "repo/1", "sess-1", &sample_transcript(),
                &["nonexistent".to_string()],
            )
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn provider_error_on_one_rubric_does_not_abort_others() {
        let rubric_store = MockOutcomesStore::new();
        let result_store = MockOutcomesStore::new();
        let client = RubricClient::new(rubric_store.clone());
        client.put(&rubric("fail-rubric", None)).await.unwrap();
        client.put(&rubric("ok-rubric", None)).await.unwrap();

        // Failing provider for both — just check that we get 0 results (not a panic)
        let provider = MockEvaluationProvider::failing("LLM down");
        let ev = Evaluator::new(provider, rubric_store, result_store);

        let results = ev
            .evaluate_session("pr", "repo/1", "sess-1", &sample_transcript(), &[])
            .await
            .unwrap();

        assert!(results.is_empty(), "errors should be swallowed, not propagated");
    }

    #[tokio::test]
    async fn returns_empty_when_no_rubrics_registered() {
        let rubric_store = MockOutcomesStore::new();
        let result_store = MockOutcomesStore::new();
        let provider = MockEvaluationProvider::returning(vec![], "");
        let ev = Evaluator::new(provider, rubric_store, result_store);

        let results = ev
            .evaluate_session("pr", "repo/1", "sess-1", &[], &[])
            .await
            .unwrap();

        assert!(results.is_empty());
    }
}
