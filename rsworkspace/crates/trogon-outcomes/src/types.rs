use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ── Rubric definition ─────────────────────────────────────────────────────────

/// A single measurable criterion within a rubric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Criterion {
    /// Short name used to identify this criterion in results.
    pub name: String,
    /// What the evaluator should look for. Be specific and actionable.
    pub description: String,
    /// Relative weight when computing the overall score (all weights need not sum to 1).
    #[serde(default = "default_weight")]
    pub weight: f32,
}

fn default_weight() -> f32 {
    1.0
}

/// A named, versioned quality rubric for evaluating agent sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rubric {
    pub id: String,
    pub name: String,
    /// Human description of what this rubric measures.
    pub description: String,
    /// When set, this rubric only applies to sessions of this actor type (e.g. `"pr"`).
    /// `None` means it applies to all actor types.
    pub actor_type_filter: Option<String>,
    pub criteria: Vec<Criterion>,
    /// Minimum weighted average score (0.0–1.0) needed to pass.
    #[serde(default = "default_passing_score")]
    pub passing_score: f32,
    pub created_at: u64,
}

fn default_passing_score() -> f32 {
    0.7
}

impl Rubric {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        criteria: Vec<Criterion>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            actor_type_filter: None,
            criteria,
            passing_score: default_passing_score(),
            created_at: now_ms(),
        }
    }

    pub fn with_actor_type_filter(mut self, filter: impl Into<String>) -> Self {
        self.actor_type_filter = Some(filter.into());
        self
    }

    pub fn with_passing_score(mut self, score: f32) -> Self {
        self.passing_score = score;
        self
    }

    /// Whether this rubric applies to the given actor type.
    pub fn matches_actor_type(&self, actor_type: &str) -> bool {
        match &self.actor_type_filter {
            None => true,
            Some(filter) => filter == actor_type,
        }
    }
}

// ── Evaluation results ────────────────────────────────────────────────────────

/// Score for a single criterion within an evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionScore {
    pub criterion: String,
    /// Score from 0.0 (completely fails) to 1.0 (fully meets the criterion).
    pub score: f32,
    pub reasoning: String,
}

/// Full evaluation result for one session against one rubric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub rubric_id: String,
    pub rubric_name: String,
    pub session_id: String,
    pub actor_type: String,
    pub actor_key: String,
    pub scores: Vec<CriterionScore>,
    /// Weighted average of criterion scores.
    pub overall_score: f32,
    pub passed: bool,
    pub evaluator_reasoning: String,
    pub evaluated_at: u64,
}

impl EvaluationResult {
    pub fn compute(
        rubric: &Rubric,
        session_id: impl Into<String>,
        actor_type: impl Into<String>,
        actor_key: impl Into<String>,
        scores: Vec<CriterionScore>,
        evaluator_reasoning: impl Into<String>,
    ) -> Self {
        let overall_score = weighted_average(rubric, &scores);
        let passed = overall_score >= rubric.passing_score;
        Self {
            rubric_id: rubric.id.clone(),
            rubric_name: rubric.name.clone(),
            session_id: session_id.into(),
            actor_type: actor_type.into(),
            actor_key: actor_key.into(),
            overall_score,
            passed,
            scores,
            evaluator_reasoning: evaluator_reasoning.into(),
            evaluated_at: now_ms(),
        }
    }
}

fn weighted_average(rubric: &Rubric, scores: &[CriterionScore]) -> f32 {
    let mut total_weight = 0.0f32;
    let mut weighted_sum = 0.0f32;

    for criterion in &rubric.criteria {
        if let Some(s) = scores.iter().find(|s| s.criterion == criterion.name) {
            weighted_sum += s.score * criterion.weight;
            total_weight += criterion.weight;
        }
    }

    if total_weight == 0.0 {
        return 0.0;
    }

    weighted_sum / total_weight
}

// ── Trigger ───────────────────────────────────────────────────────────────────

/// Payload published to `sessions.evaluate.>` to trigger evaluation.
///
/// If `rubric_ids` is empty, all rubrics matching the actor type are applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateTrigger {
    pub actor_type: String,
    pub actor_key: String,
    pub session_id: String,
    /// Specific rubric IDs to run. Empty means "run all applicable rubrics".
    #[serde(default)]
    pub rubric_ids: Vec<String>,
}

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum OutcomesError {
    Llm(String),
    Parse(String),
    Store(String),
    Transcript(String),
}

impl std::fmt::Display for OutcomesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutcomesError::Llm(e) => write!(f, "LLM error: {e}"),
            OutcomesError::Parse(e) => write!(f, "parse error: {e}"),
            OutcomesError::Store(e) => write!(f, "store error: {e}"),
            OutcomesError::Transcript(e) => write!(f, "transcript error: {e}"),
        }
    }
}

impl std::error::Error for OutcomesError {}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rubric_with_weights(weights: &[(&str, f32)]) -> Rubric {
        let criteria = weights
            .iter()
            .map(|(name, w)| Criterion {
                name: name.to_string(),
                description: format!("{name} criterion"),
                weight: *w,
            })
            .collect();
        Rubric::new("r1", "Test Rubric", "desc", criteria)
    }

    fn scores(values: &[(&str, f32)]) -> Vec<CriterionScore> {
        values
            .iter()
            .map(|(name, score)| CriterionScore {
                criterion: name.to_string(),
                score: *score,
                reasoning: String::new(),
            })
            .collect()
    }

    #[test]
    fn weighted_average_equal_weights() {
        let rubric = rubric_with_weights(&[("a", 1.0), ("b", 1.0)]);
        let s = scores(&[("a", 0.8), ("b", 0.6)]);
        let avg = weighted_average(&rubric, &s);
        assert!((avg - 0.7).abs() < 1e-5, "expected 0.7, got {avg}");
    }

    #[test]
    fn weighted_average_unequal_weights() {
        let rubric = rubric_with_weights(&[("a", 2.0), ("b", 1.0)]);
        let s = scores(&[("a", 0.9), ("b", 0.3)]);
        // (0.9*2 + 0.3*1) / 3 = 2.1/3 = 0.7
        let avg = weighted_average(&rubric, &s);
        assert!((avg - 0.7).abs() < 1e-5, "expected 0.7, got {avg}");
    }

    #[test]
    fn weighted_average_returns_zero_when_no_matching_scores() {
        let rubric = rubric_with_weights(&[("a", 1.0)]);
        let s = scores(&[("b", 0.9)]); // criterion "b" not in rubric
        assert_eq!(weighted_average(&rubric, &s), 0.0);
    }

    #[test]
    fn evaluation_result_passes_when_above_threshold() {
        let mut rubric = rubric_with_weights(&[("quality", 1.0)]);
        rubric.passing_score = 0.7;
        let s = scores(&[("quality", 0.8)]);
        let result = EvaluationResult::compute(&rubric, "sess", "pr", "repo/1", s, "good");
        assert!(result.passed);
        assert!((result.overall_score - 0.8).abs() < 1e-5);
    }

    #[test]
    fn evaluation_result_fails_when_below_threshold() {
        let mut rubric = rubric_with_weights(&[("quality", 1.0)]);
        rubric.passing_score = 0.7;
        let s = scores(&[("quality", 0.5)]);
        let result = EvaluationResult::compute(&rubric, "sess", "pr", "repo/1", s, "poor");
        assert!(!result.passed);
    }

    #[test]
    fn rubric_matches_actor_type_when_filter_is_none() {
        let rubric = Rubric::new("r", "name", "desc", vec![]);
        assert!(rubric.matches_actor_type("pr"));
        assert!(rubric.matches_actor_type("anything"));
    }

    #[test]
    fn rubric_matches_actor_type_with_filter() {
        let rubric =
            Rubric::new("r", "name", "desc", vec![]).with_actor_type_filter("pr");
        assert!(rubric.matches_actor_type("pr"));
        assert!(!rubric.matches_actor_type("issue"));
    }

    #[test]
    fn evaluate_trigger_deserializes_with_empty_rubric_ids() {
        let json = r#"{"actor_type":"pr","actor_key":"repo/1","session_id":"sess-x"}"#;
        let t: EvaluateTrigger = serde_json::from_str(json).unwrap();
        assert!(t.rubric_ids.is_empty());
    }
}
