use std::future::Future;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use serde::Deserialize;
use trogon_registry::AgentCapability;

use crate::types::{OrchestratorError, SubTask, TaskPlan};

// ── Public provider trait ─────────────────────────────────────────────────────

/// Abstraction over the LLM calls used by the orchestrator.
///
/// Two operations:
/// - `plan_task` — decompose a task description into parallel sub-tasks given
///   the set of available agent capabilities.
/// - `synthesize_results` — combine all sub-task outputs into a final answer.
pub trait OrchestratorProvider: Send + Sync + Clone + 'static {
    fn plan_task<'a>(
        &'a self,
        task: &'a str,
        capabilities: &'a [AgentCapability],
    ) -> impl Future<Output = Result<TaskPlan, OrchestratorError>> + Send + 'a;

    fn synthesize_results<'a>(
        &'a self,
        task: &'a str,
        plan: &'a TaskPlan,
        results: &'a [crate::types::SubTaskResult],
    ) -> impl Future<Output = Result<String, OrchestratorError>> + Send + 'a;
}

// ── Private HTTP client trait (injectable in tests) ───────────────────────────

trait OrchestratorHttpClient: Send + Sync + Clone + 'static {
    fn post<'a>(
        &'a self,
        url: &'a str,
        api_key: &'a str,
        auth_style: &'a OrchestratorAuthStyle,
        body: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, String>> + Send + 'a;
}

impl OrchestratorHttpClient for reqwest::Client {
    async fn post<'a>(
        &'a self,
        url: &'a str,
        api_key: &'a str,
        auth_style: &'a OrchestratorAuthStyle,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let auth_value = match auth_style {
            OrchestratorAuthStyle::XApiKey => {
                let mut v = HeaderValue::from_str(api_key).map_err(|e| e.to_string())?;
                v.set_sensitive(true);
                ("x-api-key", v)
            }
            OrchestratorAuthStyle::Bearer => {
                let bearer = format!("Bearer {api_key}");
                let mut v = HeaderValue::from_str(&bearer).map_err(|e| e.to_string())?;
                v.set_sensitive(true);
                (AUTHORIZATION.as_str(), v)
            }
        };

        let resp = self
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .header(auth_value.0, auth_value.1)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("LLM returned {status}: {text}"));
        }

        resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
    }
}

// ── LLM config ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum OrchestratorAuthStyle {
    XApiKey,
    Bearer,
}

#[derive(Debug, Clone)]
pub struct OrchestratorLlmConfig {
    pub api_url: String,
    pub api_key: String,
    pub auth_style: OrchestratorAuthStyle,
    pub model: String,
    pub max_tokens: u32,
}

// ── AnthropicOrchestratorProvider ─────────────────────────────────────────────

#[derive(Clone)]
pub struct AnthropicOrchestratorProvider<C = reqwest::Client> {
    client: C,
    config: OrchestratorLlmConfig,
}

impl AnthropicOrchestratorProvider {
    pub fn new(config: OrchestratorLlmConfig) -> Self {
        Self { client: reqwest::Client::new(), config }
    }
}

#[allow(private_bounds)]
impl<C: OrchestratorHttpClient> AnthropicOrchestratorProvider<C> {
    #[cfg(test)]
    fn with_client(config: OrchestratorLlmConfig, client: C) -> Self {
        Self { client, config }
    }

    async fn call_llm(&self, prompt: String) -> Result<String, OrchestratorError> {
        let body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": [{"role": "user", "content": prompt}]
        });

        let resp = self
            .client
            .post(&self.config.api_url, &self.config.api_key, &self.config.auth_style, body)
            .await
            .map_err(OrchestratorError::Planning)?;

        extract_text_content(&resp)
    }
}

#[allow(private_bounds)]
impl<C: OrchestratorHttpClient> OrchestratorProvider for AnthropicOrchestratorProvider<C> {
    async fn plan_task<'a>(
        &'a self,
        task: &'a str,
        capabilities: &'a [AgentCapability],
    ) -> Result<TaskPlan, OrchestratorError> {
        let caps_json = serde_json::to_string_pretty(capabilities)
            .unwrap_or_else(|_| "[]".to_string());
        let prompt = format!(
            "You are an orchestrator. Break down the following task into parallel sub-tasks, \
             each assigned to one of the available agent capabilities.\n\n\
             <task>{task}</task>\n\n\
             <available_agents>{caps_json}</available_agents>\n\n\
             Respond with a JSON object matching exactly this schema:\n\
             {{\"subtasks\":[{{\"id\":\"<uuid>\",\"capability\":\"<agent_capability_name>\",\
             \"description\":\"<what_to_do>\",\"payload\":\"<json_string_payload>\"}},...],\
             \"reasoning\":\"<why_this_decomposition>\"}}\n\
             Use only capabilities from the available_agents list. \
             The payload field should be a JSON string the target agent will receive."
        );

        let text = self.call_llm(prompt).await.map_err(|e| OrchestratorError::Planning(e.to_string()))?;
        parse_plan(&text)
    }

    async fn synthesize_results<'a>(
        &'a self,
        task: &'a str,
        plan: &'a TaskPlan,
        results: &'a [crate::types::SubTaskResult],
    ) -> Result<String, OrchestratorError> {
        let results_text = format_results_for_prompt(plan, results);
        let prompt = format!(
            "You are an orchestrator synthesizing results from multiple agents.\n\n\
             <original_task>{task}</original_task>\n\n\
             <plan_reasoning>{}</plan_reasoning>\n\n\
             <agent_results>{results_text}</agent_results>\n\n\
             Synthesize a comprehensive, coherent response that addresses the original task \
             using all agent outputs. Be concise and direct.",
            plan.reasoning
        );

        self.call_llm(prompt).await.map_err(|e| OrchestratorError::Synthesis(e.to_string()))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_text_content(resp: &serde_json::Value) -> Result<String, OrchestratorError> {
    resp["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| OrchestratorError::Planning("unexpected LLM response shape".into()))
}

#[derive(Deserialize)]
struct RawPlan {
    subtasks: Vec<RawSubTask>,
    reasoning: String,
}

#[derive(Deserialize)]
struct RawSubTask {
    id: String,
    capability: String,
    description: String,
    payload: String,
}

fn parse_plan(text: &str) -> Result<TaskPlan, OrchestratorError> {
    let json_str = extract_json_block(text);
    let raw: RawPlan = serde_json::from_str(json_str)
        .map_err(|e| OrchestratorError::Planning(format!("plan parse error: {e}")))?;

    let subtasks = raw
        .subtasks
        .into_iter()
        .map(|s| SubTask {
            id: s.id,
            capability: s.capability,
            description: s.description,
            payload: s.payload.into_bytes(),
        })
        .collect();

    Ok(TaskPlan { subtasks, reasoning: raw.reasoning })
}

fn extract_json_block(text: &str) -> &str {
    // Strip markdown code fences if present
    if let Some(start) = text.find("```json") {
        let content = &text[start + 7..];
        if let Some(end) = content.find("```") {
            return content[..end].trim();
        }
    }
    if let Some(start) = text.find("```") {
        let content = &text[start + 3..];
        if let Some(end) = content.find("```") {
            return content[..end].trim();
        }
    }
    // Otherwise try to find the outermost JSON object
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            return &text[start..=end];
        }
    }
    text.trim()
}

fn format_results_for_prompt(plan: &TaskPlan, results: &[crate::types::SubTaskResult]) -> String {
    let mut out = String::new();
    for result in results {
        let description = plan
            .subtasks
            .iter()
            .find(|s| s.id == result.subtask_id)
            .map(|s| s.description.as_str())
            .unwrap_or("unknown subtask");
        let output = if result.success {
            String::from_utf8_lossy(&result.output).into_owned()
        } else {
            format!("[ERROR: {}]", result.error.as_deref().unwrap_or("unknown"))
        };
        out.push_str(&format!(
            "<result capability=\"{}\" task=\"{}\">{}</result>\n",
            result.capability, description, output
        ));
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SubTaskResult, TaskPlan};
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct MockHttpClient {
        responses: Arc<Mutex<Vec<Result<serde_json::Value, String>>>>,
    }

    impl MockHttpClient {
        fn with_responses(responses: Vec<Result<serde_json::Value, String>>) -> Self {
            Self { responses: Arc::new(Mutex::new(responses)) }
        }
    }

    impl OrchestratorHttpClient for MockHttpClient {
        fn post<'a>(
            &'a self,
            _url: &'a str,
            _api_key: &'a str,
            _auth_style: &'a OrchestratorAuthStyle,
            _body: serde_json::Value,
        ) -> impl Future<Output = Result<serde_json::Value, String>> + Send + 'a {
            let mut responses = self.responses.lock().unwrap();
            let response = if responses.is_empty() {
                Err("no more responses".to_string())
            } else {
                responses.remove(0)
            };
            async move { response }
        }
    }

    fn config() -> OrchestratorLlmConfig {
        OrchestratorLlmConfig {
            api_url: "http://test".into(),
            api_key: "key".into(),
            auth_style: OrchestratorAuthStyle::XApiKey,
            model: "claude-sonnet-4-6".into(),
            max_tokens: 1024,
        }
    }

    fn text_response(text: &str) -> serde_json::Value {
        serde_json::json!({"content": [{"type": "text", "text": text}]})
    }

    #[tokio::test]
    async fn plan_task_parses_json_response() {
        let plan_json = r#"{"subtasks":[{"id":"1","capability":"code_review","description":"Review the PR","payload":"{}"}],"reasoning":"Need code review"}"#;
        let client = MockHttpClient::with_responses(vec![Ok(text_response(plan_json))]);
        let provider = AnthropicOrchestratorProvider::with_client(config(), client);

        let caps = vec![AgentCapability::new("PrActor", ["code_review"], "actors.pr.>")];
        let plan = provider.plan_task("Review PR #123", &caps).await.unwrap();

        assert_eq!(plan.subtasks.len(), 1);
        assert_eq!(plan.subtasks[0].capability, "code_review");
        assert_eq!(plan.reasoning, "Need code review");
    }

    #[tokio::test]
    async fn plan_task_strips_markdown_fences() {
        let plan_json = "```json\n{\"subtasks\":[{\"id\":\"1\",\"capability\":\"code_review\",\"description\":\"Review\",\"payload\":\"{}\"}],\"reasoning\":\"ok\"}\n```";
        let client = MockHttpClient::with_responses(vec![Ok(text_response(plan_json))]);
        let provider = AnthropicOrchestratorProvider::with_client(config(), client);

        let caps = vec![AgentCapability::new("PrActor", ["code_review"], "actors.pr.>")];
        let plan = provider.plan_task("Review PR", &caps).await.unwrap();
        assert_eq!(plan.subtasks.len(), 1);
    }

    #[tokio::test]
    async fn synthesize_results_returns_text() {
        let synthesis = "Overall: the code looks good.";
        let client = MockHttpClient::with_responses(vec![Ok(text_response(synthesis))]);
        let provider = AnthropicOrchestratorProvider::with_client(config(), client);

        let plan = TaskPlan {
            subtasks: vec![],
            reasoning: "single pass".into(),
        };
        let results = vec![SubTaskResult::ok("1", "code_review", b"LGTM".to_vec())];

        let result = provider.synthesize_results("Review PR", &plan, &results).await.unwrap();
        assert_eq!(result, synthesis);
    }

    #[tokio::test]
    async fn plan_task_returns_error_on_bad_json() {
        let client =
            MockHttpClient::with_responses(vec![Ok(text_response("not valid json at all"))]);
        let provider = AnthropicOrchestratorProvider::with_client(config(), client);
        let caps = vec![AgentCapability::new("PrActor", ["code_review"], "actors.pr.>")];
        let err = provider.plan_task("task", &caps).await.unwrap_err();
        assert!(matches!(err, OrchestratorError::Planning(_)));
    }

    #[tokio::test]
    async fn extract_text_content_fails_on_empty_response() {
        let resp = serde_json::json!({});
        let err = extract_text_content(&resp).unwrap_err();
        assert!(matches!(err, OrchestratorError::Planning(_)));
    }

    // ── extract_json_block unit tests ─────────────────────────────────────────

    #[test]
    fn extract_json_block_returns_plain_json_unchanged() {
        let json = r#"{"subtasks":[],"reasoning":"ok"}"#;
        assert_eq!(extract_json_block(json), json);
    }

    #[test]
    fn extract_json_block_extracts_from_trailing_prose() {
        // JSON followed by explanatory text: rfind('}') picks up the JSON's
        // closing brace correctly.
        let text = r#"{"subtasks":[],"reasoning":"ok"} That's the plan."#;
        assert_eq!(extract_json_block(text), r#"{"subtasks":[],"reasoning":"ok"}"#);
    }

    #[test]
    fn extract_json_block_with_braces_in_leading_prose_returns_mangled_slice() {
        // When prose before the JSON block itself contains '{', find('{') picks
        // the wrong start. The returned slice is not valid JSON — callers
        // surface this as a parse error (Planning error), not silent data loss.
        let text = r#"Here {is} the plan: {"subtasks":[],"reasoning":"ok"}"#;
        let extracted = extract_json_block(text);
        // The extraction is wrong (starts at `{is}`), so parsing should fail.
        assert!(serde_json::from_str::<TaskPlan>(extracted).is_err());
    }

    #[test]
    fn extract_json_block_prefers_code_fence_over_brace_search() {
        // ```json fences take priority over the brace-search fallback.
        let text = "preamble {broken} ```json\n{\"subtasks\":[],\"reasoning\":\"ok\"}\n```";
        let extracted = extract_json_block(text);
        assert!(serde_json::from_str::<TaskPlan>(extracted).is_ok());
    }
}
