use std::future::Future;

use trogon_transcript::entry::Role;
use trogon_transcript::TranscriptEntry;

use crate::{
    evaluator::Evaluator,
    provider::EvaluationProvider,
    store::OutcomesStore,
    types::{EvaluationResult, OutcomesError, now_ms},
};

/// An agent that can be driven by the Ralph loop.
///
/// `task` is the original prompt. `feedback` carries the grader's notes from
/// the previous failed attempt so the agent can improve.
pub trait TaskExecutor: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        task: &'a str,
        feedback: Option<&'a str>,
    ) -> impl Future<Output = Result<String, RalphLoopError>> + Send + 'a;
}

/// Record of one agent attempt and its evaluation.
#[derive(Debug, Clone)]
pub struct LoopIteration {
    /// Zero-based attempt index.
    pub attempt: usize,
    /// Raw output returned by the executor for this attempt.
    pub output: String,
    /// Evaluation result produced by the isolated grader.
    pub result: EvaluationResult,
}

/// Final outcome of a complete Ralph loop run.
#[derive(Debug, Clone)]
pub struct RalphLoopResult {
    pub iterations: Vec<LoopIteration>,
    pub final_output: String,
    pub passed: bool,
}

#[derive(Debug)]
pub enum RalphLoopError {
    Executor(String),
    Evaluation(OutcomesError),
}

impl std::fmt::Display for RalphLoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RalphLoopError::Executor(e) => write!(f, "executor error: {e}"),
            RalphLoopError::Evaluation(e) => write!(f, "evaluation error: {e}"),
        }
    }
}

impl std::error::Error for RalphLoopError {}

impl From<OutcomesError> for RalphLoopError {
    fn from(e: OutcomesError) -> Self {
        RalphLoopError::Evaluation(e)
    }
}

/// Runs an agent in a verify-and-improve loop against a rubric.
///
/// Each iteration:
/// 1. Calls the executor with the task (and grader feedback from the previous attempt).
/// 2. Evaluates the output via an isolated grader (`Evaluator`).
/// 3. Stops if the output passes the rubric.
/// 4. Otherwise formats improvement notes and repeats.
///
/// Stops after `max_iterations` regardless of pass/fail status.
pub struct RalphLoop<E: TaskExecutor, P: EvaluationProvider, S: OutcomesStore> {
    executor: E,
    evaluator: Evaluator<P, S>,
    max_iterations: usize,
}

impl<E: TaskExecutor, P: EvaluationProvider, S: OutcomesStore> RalphLoop<E, P, S> {
    pub fn new(executor: E, evaluator: Evaluator<P, S>, max_iterations: usize) -> Self {
        assert!(max_iterations > 0, "max_iterations must be at least 1");
        Self { executor, evaluator, max_iterations }
    }

    /// Run the loop for `task` against `rubric_id`.
    ///
    /// If `rubric_id` is `None`, all rubrics matching `actor_type` are applied.
    /// If no rubric matches the actor type the output is treated as passing
    /// immediately (no rubric = no constraint).
    pub async fn run(
        &self,
        task: &str,
        actor_type: &str,
        actor_key: &str,
        rubric_id: Option<&str>,
    ) -> Result<RalphLoopResult, RalphLoopError> {
        let rubric_ids: Vec<String> =
            rubric_id.map(|id| vec![id.to_string()]).unwrap_or_default();
        let mut feedback: Option<String> = None;
        let mut iterations = Vec::new();

        for attempt in 0..self.max_iterations {
            let output = self.executor.execute(task, feedback.as_deref()).await?;

            let session_id = format!("ralph-{actor_type}-{attempt}");
            let transcript = synthetic_transcript(task, &output);

            let results = self
                .evaluator
                .evaluate_session(actor_type, actor_key, &session_id, &transcript, &rubric_ids)
                .await
                .map_err(RalphLoopError::Evaluation)?;

            if results.is_empty() {
                // No rubric matched — unconstrained output, treat as passed.
                return Ok(RalphLoopResult { iterations, final_output: output, passed: true });
            }

            // The worst-scoring rubric determines whether the attempt passed.
            let worst = results
                .into_iter()
                .min_by(|a, b| {
                    a.overall_score
                        .partial_cmp(&b.overall_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .expect("non-empty results");

            let passed = worst.passed;
            feedback = Some(format_feedback(attempt + 1, &worst));
            iterations.push(LoopIteration { attempt, output: output.clone(), result: worst });

            if passed {
                return Ok(RalphLoopResult {
                    iterations,
                    final_output: output,
                    passed: true,
                });
            }
        }

        let final_output = iterations.last().map(|i| i.output.clone()).unwrap_or_default();
        Ok(RalphLoopResult { iterations, final_output, passed: false })
    }
}

/// Build a minimal two-turn transcript from a task prompt and agent output.
fn synthetic_transcript(task: &str, output: &str) -> Vec<TranscriptEntry> {
    let now = now_ms();
    vec![
        TranscriptEntry::Message {
            role: Role::User,
            content: task.to_string(),
            timestamp: now,
            tokens: None,
        },
        TranscriptEntry::Message {
            role: Role::Assistant,
            content: output.to_string(),
            timestamp: now,
            tokens: None,
        },
    ]
}

/// Format evaluation results into actionable feedback for the next attempt.
fn format_feedback(attempt: usize, result: &EvaluationResult) -> String {
    let mut out = format!(
        "Attempt {attempt} did not meet the quality bar (score: {:.0}%).\n\nCriterion scores:\n",
        result.overall_score * 100.0,
    );
    for score in &result.scores {
        out.push_str(&format!(
            "- {}: {:.0}% — {}\n",
            score.criterion,
            score.score * 100.0,
            score.reasoning,
        ));
    }
    out.push_str("\nRevise your response to address the weak areas.");
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::store::mock::MockOutcomesStore;
    use crate::store::RubricClient;
    use crate::types::{Criterion, CriterionScore, Rubric};

    // ── MockExecutor ──────────────────────────────────────────────────────────

    #[derive(Clone)]
    struct MockExecutor {
        responses: Arc<Mutex<Vec<String>>>,
        received_feedback: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl MockExecutor {
        fn with_responses(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses.into_iter().map(Into::into).collect())),
                received_feedback: Arc::new(Mutex::new(vec![])),
            }
        }
    }

    impl TaskExecutor for MockExecutor {
        fn execute<'a>(
            &'a self,
            _task: &'a str,
            feedback: Option<&'a str>,
        ) -> impl Future<Output = Result<String, RalphLoopError>> + Send + 'a {
            let mut r = self.responses.lock().unwrap();
            let output = if r.is_empty() {
                "default output".to_string()
            } else {
                r.remove(0)
            };
            self.received_feedback.lock().unwrap().push(feedback.map(str::to_owned));
            async move { Ok(output) }
        }
    }

    // ── MockEvaluationProvider ────────────────────────────────────────────────

    #[derive(Clone)]
    struct SequencedProvider {
        // Each call pops one result; if empty, defaults to passing (1.0).
        scores: Arc<Mutex<Vec<Vec<CriterionScore>>>>,
    }

    impl SequencedProvider {
        fn returning(rounds: Vec<Vec<CriterionScore>>) -> Self {
            Self { scores: Arc::new(Mutex::new(rounds)) }
        }
    }

    impl crate::provider::EvaluationProvider for SequencedProvider {
        fn evaluate<'a>(
            &'a self,
            _rubric: &'a Rubric,
            _transcript: &'a [TranscriptEntry],
        ) -> impl Future<Output = Result<(Vec<CriterionScore>, String), OutcomesError>> + Send + 'a
        {
            let mut s = self.scores.lock().unwrap();
            let scores = if s.is_empty() {
                vec![CriterionScore {
                    criterion: "quality".into(),
                    score: 1.0,
                    reasoning: "perfect".into(),
                }]
            } else {
                s.remove(0)
            };
            async move { Ok((scores, "grader reasoning".into())) }
        }
    }

    fn rubric(id: &str) -> Rubric {
        Rubric::new(
            id,
            "Quality",
            "Measures quality",
            vec![Criterion {
                name: "quality".into(),
                description: "Is the output good?".into(),
                weight: 1.0,
            }],
        )
        .with_passing_score(0.7)
    }

    fn failing_scores() -> Vec<CriterionScore> {
        vec![CriterionScore {
            criterion: "quality".into(),
            score: 0.4,
            reasoning: "needs improvement".into(),
        }]
    }

    fn passing_scores() -> Vec<CriterionScore> {
        vec![CriterionScore {
            criterion: "quality".into(),
            score: 0.9,
            reasoning: "excellent".into(),
        }]
    }

    async fn loop_with_rubric(
        executor: MockExecutor,
        provider: SequencedProvider,
        max: usize,
    ) -> RalphLoop<MockExecutor, SequencedProvider, MockOutcomesStore> {
        let rubric_store = MockOutcomesStore::new();
        let result_store = MockOutcomesStore::new();
        let client = RubricClient::new(rubric_store.clone());
        client.put(&rubric("r1")).await.unwrap();
        let evaluator = Evaluator::new(provider, rubric_store, result_store);
        RalphLoop::new(executor, evaluator, max)
    }

    #[tokio::test]
    async fn stops_on_first_pass() {
        let executor = MockExecutor::with_responses(["good output"]);
        let provider = SequencedProvider::returning(vec![passing_scores()]);
        let lp = loop_with_rubric(executor, provider, 5).await;

        let result = lp.run("do a task", "pr", "repo/1", Some("r1")).await.unwrap();

        assert!(result.passed);
        assert_eq!(result.iterations.len(), 1);
        assert_eq!(result.final_output, "good output");
        assert_eq!(result.iterations[0].attempt, 0);
    }

    #[tokio::test]
    async fn retries_until_pass() {
        let executor =
            MockExecutor::with_responses(["bad output", "better output", "great output"]);
        let provider = SequencedProvider::returning(vec![
            failing_scores(),
            failing_scores(),
            passing_scores(),
        ]);
        let lp = loop_with_rubric(executor, provider, 5).await;

        let result = lp.run("do a task", "pr", "repo/1", Some("r1")).await.unwrap();

        assert!(result.passed);
        assert_eq!(result.iterations.len(), 3);
        assert_eq!(result.final_output, "great output");
    }

    #[tokio::test]
    async fn stops_at_max_iterations_if_never_passes() {
        let executor = MockExecutor::with_responses(["out1", "out2", "out3"]);
        let provider = SequencedProvider::returning(vec![
            failing_scores(),
            failing_scores(),
            failing_scores(),
        ]);
        let lp = loop_with_rubric(executor, provider, 3).await;

        let result = lp.run("do a task", "pr", "repo/1", Some("r1")).await.unwrap();

        assert!(!result.passed);
        assert_eq!(result.iterations.len(), 3);
        assert_eq!(result.final_output, "out3");
    }

    #[tokio::test]
    async fn passes_feedback_on_retry() {
        let executor = MockExecutor::with_responses(["bad", "good"]);
        let fb_tracker = executor.received_feedback.clone();
        let provider =
            SequencedProvider::returning(vec![failing_scores(), passing_scores()]);
        let lp = loop_with_rubric(executor, provider, 3).await;

        lp.run("do a task", "pr", "repo/1", Some("r1")).await.unwrap();

        let feedback = fb_tracker.lock().unwrap().clone();
        assert_eq!(feedback.len(), 2);
        assert!(feedback[0].is_none(), "first attempt has no prior feedback");
        assert!(
            feedback[1].as_deref().unwrap_or("").contains("Attempt 1"),
            "second attempt should include feedback from attempt 1"
        );
    }

    #[tokio::test]
    async fn passes_when_no_rubric_matches_actor_type() {
        let rubric_store = MockOutcomesStore::new();
        let result_store = MockOutcomesStore::new();
        let client = RubricClient::new(rubric_store.clone());
        // Register a rubric filtered to "issue" — won't match "pr".
        client
            .put(&rubric("r1").with_actor_type_filter("issue"))
            .await
            .unwrap();
        let provider = SequencedProvider::returning(vec![]);
        let evaluator = Evaluator::new(provider, rubric_store, result_store);
        let executor = MockExecutor::with_responses(["output"]);
        let lp = RalphLoop::new(executor, evaluator, 3);

        let result = lp.run("task", "pr", "repo/1", None).await.unwrap();

        assert!(result.passed, "no matching rubric should be treated as passed");
        assert!(result.iterations.is_empty());
    }

    #[tokio::test]
    async fn max_iterations_one_runs_exactly_once() {
        let executor = MockExecutor::with_responses(["single attempt"]);
        let provider = SequencedProvider::returning(vec![failing_scores()]);
        let lp = loop_with_rubric(executor, provider, 1).await;

        let result = lp.run("task", "pr", "repo/1", Some("r1")).await.unwrap();

        assert!(!result.passed);
        assert_eq!(result.iterations.len(), 1);
        assert_eq!(result.final_output, "single attempt");
    }

    #[tokio::test]
    async fn feedback_contains_criterion_scores() {
        let executor = MockExecutor::with_responses(["bad", "ok"]);
        let fb_tracker = executor.received_feedback.clone();
        let provider = SequencedProvider::returning(vec![
            vec![CriterionScore {
                criterion: "clarity".into(),
                score: 0.3,
                reasoning: "too verbose".into(),
            }],
            passing_scores(),
        ]);
        let rubric_store = MockOutcomesStore::new();
        let result_store = MockOutcomesStore::new();
        let client = RubricClient::new(rubric_store.clone());
        client
            .put(&Rubric::new(
                "r1",
                "Clarity",
                "Is it clear?",
                vec![Criterion {
                    name: "clarity".into(),
                    description: "clear?".into(),
                    weight: 1.0,
                }],
            ))
            .await
            .unwrap();
        let evaluator = Evaluator::new(provider, rubric_store, result_store);
        let lp = RalphLoop::new(executor, evaluator, 3);

        lp.run("task", "pr", "repo/1", Some("r1")).await.unwrap();

        let feedback_on_retry = fb_tracker.lock().unwrap()[1].clone().unwrap();
        assert!(feedback_on_retry.contains("clarity"), "feedback should name the criterion");
        assert!(feedback_on_retry.contains("too verbose"), "feedback should include reasoning");
    }
}
