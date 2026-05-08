//! LLM provider for session evaluation.
//!
//! Same pattern as `trogon-memory/src/provider.rs`: private HTTP client trait
//! for testability, public `EvaluationProvider` trait, concrete
//! `AnthropicEvaluationProvider<C>`.

use std::future::Future;

use serde::{Deserialize, Serialize};
use trogon_transcript::TranscriptEntry;

use crate::types::{CriterionScore, OutcomesError, Rubric};

// ── Prompts ───────────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "\
You are an objective evaluation assistant. Your task is to score an agent \
session transcript against a quality rubric. \
For each criterion, assign a score from 0.0 (completely fails) to 1.0 (fully meets), \
and provide a concise reasoning for that score. \
Also provide a brief overall reasoning summarizing the session quality. \
Output ONLY valid JSON in exactly this format — no markdown fences, no explanation: \
{\"scores\":[{\"criterion\":\"<name>\",\"score\":<0.0-1.0>,\"reasoning\":\"<text>\"},...],\
\"overall_reasoning\":\"<text>\"}";

// ── LLM configuration ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub enum EvalAuthStyle {
    #[default]
    XApiKey,
    Bearer,
}

#[derive(Debug, Clone)]
pub struct EvalLlmConfig {
    pub api_url: String,
    pub api_key: String,
    pub auth_style: EvalAuthStyle,
    pub model: String,
    pub max_tokens: u32,
}

impl Default for EvalLlmConfig {
    fn default() -> Self {
        Self {
            api_url: "https://api.anthropic.com/v1/messages".into(),
            api_key: String::new(),
            auth_style: EvalAuthStyle::XApiKey,
            model: "claude-haiku-4-5-20251001".into(),
            max_tokens: 4_096,
        }
    }
}

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct EvalRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'static str,
    messages: [EvalMessage<'a>; 1],
}

#[derive(Serialize)]
struct EvalMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
pub(crate) struct EvalResponse {
    pub(crate) stop_reason: String,
    pub(crate) content: Vec<EvalBlock>,
}

#[derive(Deserialize)]
pub(crate) struct EvalBlock {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) text: String,
}

/// LLM-returned JSON structure.
#[derive(Deserialize)]
pub(crate) struct EvalOutput {
    pub(crate) scores: Vec<CriterionScore>,
    pub(crate) overall_reasoning: String,
}

// ── HTTP abstraction ──────────────────────────────────────────────────────────

pub(crate) trait EvalHttpClient: Send + Sync {
    fn call<'a>(
        &'a self,
        url: &'a str,
        auth_header: &'a str,
        auth_value: &'a str,
        request: &'a EvalRequest<'a>,
    ) -> impl Future<Output = Result<EvalResponse, OutcomesError>> + Send + 'a;
}

impl EvalHttpClient for reqwest::Client {
    fn call<'a>(
        &'a self,
        url: &'a str,
        auth_header: &'a str,
        auth_value: &'a str,
        request: &'a EvalRequest<'a>,
    ) -> impl Future<Output = Result<EvalResponse, OutcomesError>> + Send + 'a {
        async move {
            self.post(url)
                .header(auth_header, auth_value)
                .header("anthropic-version", "2023-06-01")
                .json(request)
                .send()
                .await
                .map_err(|e| OutcomesError::Llm(e.to_string()))?
                .error_for_status()
                .map_err(|e| OutcomesError::Llm(e.to_string()))?
                .json::<EvalResponse>()
                .await
                .map_err(|e| OutcomesError::Llm(e.to_string()))
        }
    }
}

// ── EvaluationProvider (public trait) ────────────────────────────────────────

pub trait EvaluationProvider: Send + Sync {
    fn evaluate<'a>(
        &'a self,
        rubric: &'a Rubric,
        transcript: &'a [TranscriptEntry],
    ) -> impl Future<Output = Result<(Vec<CriterionScore>, String), OutcomesError>> + Send + 'a;
}

// ── AnthropicEvaluationProvider ───────────────────────────────────────────────

pub struct AnthropicEvaluationProvider<C = reqwest::Client> {
    config: EvalLlmConfig,
    client: C,
}

impl AnthropicEvaluationProvider<reqwest::Client> {
    pub fn new(config: EvalLlmConfig) -> Self {
        Self { config, client: reqwest::Client::new() }
    }
}

#[allow(private_bounds)]
impl<C: EvalHttpClient> AnthropicEvaluationProvider<C> {
    pub fn with_client(config: EvalLlmConfig, client: C) -> Self {
        Self { config, client }
    }
}

#[allow(private_bounds)]
impl<C: EvalHttpClient> EvaluationProvider for AnthropicEvaluationProvider<C> {
    fn evaluate<'a>(
        &'a self,
        rubric: &'a Rubric,
        transcript: &'a [TranscriptEntry],
    ) -> impl Future<Output = Result<(Vec<CriterionScore>, String), OutcomesError>> + Send + 'a {
        evaluate(rubric, transcript, &self.config, &self.client)
    }
}

// ── Core evaluation logic ─────────────────────────────────────────────────────

pub(crate) async fn evaluate<C: EvalHttpClient>(
    rubric: &Rubric,
    transcript: &[TranscriptEntry],
    config: &EvalLlmConfig,
    client: &C,
) -> Result<(Vec<CriterionScore>, String), OutcomesError> {
    let prompt = build_prompt(rubric, transcript);

    let req = EvalRequest {
        model: &config.model,
        max_tokens: config.max_tokens,
        system: SYSTEM_PROMPT,
        messages: [EvalMessage { role: "user", content: &prompt }],
    };

    let (auth_header, auth_value) = match config.auth_style {
        EvalAuthStyle::XApiKey => ("x-api-key", config.api_key.clone()),
        EvalAuthStyle::Bearer => ("Authorization", format!("Bearer {}", config.api_key)),
    };

    let resp = client.call(&config.api_url, auth_header, &auth_value, &req).await?;

    if resp.stop_reason != "end_turn" {
        return Err(OutcomesError::Llm(format!(
            "unexpected stop_reason: {}",
            resp.stop_reason
        )));
    }

    let text = resp
        .content
        .into_iter()
        .filter(|b| b.kind == "text")
        .map(|b| b.text)
        .collect::<Vec<_>>()
        .join("");

    let output: EvalOutput = serde_json::from_str(text.trim())
        .map_err(|e| OutcomesError::Parse(format!("{e}: {text}")))?;

    Ok((output.scores, output.overall_reasoning))
}

fn build_prompt(rubric: &Rubric, transcript: &[TranscriptEntry]) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "<rubric>\nName: {}\nDescription: {}\n\nCriteria:\n",
        rubric.name, rubric.description
    ));
    for c in &rubric.criteria {
        out.push_str(&format!("- {} (weight {}): {}\n", c.name, c.weight, c.description));
    }
    out.push_str("</rubric>\n\n<transcript>\n");

    for entry in transcript {
        match entry {
            TranscriptEntry::Message { role, content, .. } => {
                out.push_str(&format!("[{role:?}] {content}\n"));
            }
            TranscriptEntry::ToolCall { name, input, output, .. } => {
                out.push_str(&format!("[tool:{name}] input={input} output={output}\n"));
            }
            TranscriptEntry::RoutingDecision { from, to, reasoning, .. } => {
                out.push_str(&format!("[route] {from} → {to}: {reasoning}\n"));
            }
            TranscriptEntry::SubAgentSpawn { parent, child, capability, .. } => {
                out.push_str(&format!("[spawn] {parent} → {child} ({capability})\n"));
            }
        }
    }
    out.push_str("</transcript>");
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::types::{Criterion, now_ms};
    use trogon_transcript::entry::Role;

    fn rubric() -> Rubric {
        Rubric::new(
            "r1",
            "Code Review Quality",
            "Evaluates the quality of code review feedback",
            vec![
                Criterion { name: "thoroughness".into(), description: "Covers all issues".into(), weight: 1.0 },
                Criterion { name: "clarity".into(), description: "Clear explanations".into(), weight: 1.0 },
            ],
        )
    }

    fn transcript() -> Vec<TranscriptEntry> {
        vec![
            TranscriptEntry::Message {
                role: Role::User,
                content: "Please review this PR.".into(),
                timestamp: now_ms(),
                tokens: None,
            },
            TranscriptEntry::Message {
                role: Role::Assistant,
                content: "The code looks good. One minor nit on line 42.".into(),
                timestamp: now_ms(),
                tokens: None,
            },
        ]
    }

    // ── MockEvalHttpClient ────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockEvalHttpClient {
        responses: Mutex<VecDeque<Result<EvalResponse, OutcomesError>>>,
    }

    impl MockEvalHttpClient {
        fn enqueue_json(self, json: &str) -> Self {
            self.responses.lock().unwrap().push_back(Ok(EvalResponse {
                stop_reason: "end_turn".into(),
                content: vec![EvalBlock { kind: "text".into(), text: json.to_string() }],
            }));
            self
        }

        fn enqueue_err(self, msg: &str) -> Self {
            self.responses
                .lock()
                .unwrap()
                .push_back(Err(OutcomesError::Llm(msg.to_string())));
            self
        }
    }

    impl EvalHttpClient for MockEvalHttpClient {
        fn call<'a>(
            &'a self,
            _url: &'a str,
            _auth_header: &'a str,
            _auth_value: &'a str,
            _request: &'a EvalRequest<'a>,
        ) -> impl Future<Output = Result<EvalResponse, OutcomesError>> + Send + 'a {
            let result = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(EvalResponse {
                    stop_reason: "end_turn".into(),
                    content: vec![EvalBlock {
                        kind: "text".into(),
                        text: r#"{"scores":[],"overall_reasoning":""}"#.into(),
                    }],
                }));
            async move { result }
        }
    }

    fn config() -> EvalLlmConfig {
        EvalLlmConfig { api_url: "http://unused".into(), api_key: "test".into(), ..Default::default() }
    }

    #[tokio::test]
    async fn evaluate_parses_scores_and_reasoning() {
        let json = r#"{
            "scores": [
                {"criterion":"thoroughness","score":0.9,"reasoning":"covered all issues"},
                {"criterion":"clarity","score":0.8,"reasoning":"well explained"}
            ],
            "overall_reasoning":"good review overall"
        }"#;
        let mock = MockEvalHttpClient::default().enqueue_json(json);
        let (scores, reasoning) =
            evaluate(&rubric(), &transcript(), &config(), &mock).await.unwrap();
        assert_eq!(scores.len(), 2);
        assert_eq!(scores[0].criterion, "thoroughness");
        assert!((scores[0].score - 0.9).abs() < 1e-5);
        assert_eq!(reasoning, "good review overall");
    }

    #[tokio::test]
    async fn evaluate_propagates_http_error() {
        let mock = MockEvalHttpClient::default().enqueue_err("connection refused");
        let err = evaluate(&rubric(), &transcript(), &config(), &mock).await.unwrap_err();
        assert!(matches!(err, OutcomesError::Llm(_)));
    }

    #[tokio::test]
    async fn evaluate_returns_parse_error_on_invalid_json() {
        let mock = MockEvalHttpClient::default().enqueue_json("not json");
        let err = evaluate(&rubric(), &transcript(), &config(), &mock).await.unwrap_err();
        assert!(matches!(err, OutcomesError::Parse(_)));
    }

    #[test]
    fn build_prompt_includes_rubric_and_transcript() {
        let p = build_prompt(&rubric(), &transcript());
        assert!(p.contains("Code Review Quality"));
        assert!(p.contains("thoroughness"));
        assert!(p.contains("Please review this PR."));
        assert!(p.contains("<rubric>"));
        assert!(p.contains("<transcript>"));
    }
}
