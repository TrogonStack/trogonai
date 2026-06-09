//! LLM-backed safety classifier for `auto` permission mode.
//!
//! Given a side-effecting tool call (reads and protected paths are decided before
//! the classifier is consulted), asks a small model whether it is safe to
//! auto-approve and maps the reply to [`ClassifierVerdict`]. The call mirrors the
//! runner's Anthropic Messages request (proxy URL + `Authorization: Bearer` +
//! `anthropic-version`). It **fails safe**: any network/parse error or ambiguous
//! reply yields `Prompt`, so `auto` mode never silently allows on uncertainty.

use serde_json::{json, Value};

use std::sync::Arc;

use crate::permission::{ClassifierVerdict, SafetyClassifier};

const SYSTEM_PROMPT: &str = "You are a security classifier for an autonomous coding agent. \
Given one tool call, decide whether it is safe to auto-approve WITHOUT a human. \
Reply with EXACTLY one word: ALLOW if it is clearly safe (no destructive change, \
no data exfiltration, no secret access, no irreversible or network-side-effect risk); \
DENY if it is clearly dangerous (destructive, exfiltrating, or irreversible); \
PROMPT if you are at all unsure. When in doubt, answer PROMPT.";

/// Maximum characters of the tool input we include in the classification prompt.
const MAX_INPUT_CHARS: usize = 2000;

/// LLM classifier that calls the Anthropic Messages API via the runner's proxy.
pub struct LlmSafetyClassifier {
    http: reqwest::Client,
    url: String,
    token: String,
    model: String,
    extra_headers: Vec<(String, String)>,
}

impl LlmSafetyClassifier {
    /// `proxy_url` is the secret-proxy base; `anthropic_base_url` overrides it when
    /// set (matching the agent loop). `model` should be a small/cheap model.
    pub fn new(
        http: reqwest::Client,
        proxy_url: &str,
        anthropic_base_url: Option<&str>,
        token: impl Into<String>,
        model: impl Into<String>,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        let url = match anthropic_base_url {
            Some(base) => format!("{base}/messages"),
            None => format!("{proxy_url}/anthropic/v1/messages"),
        };
        Self {
            http,
            url,
            token: token.into(),
            model: model.into(),
            extra_headers,
        }
    }

    async fn do_classify(&self, tool_name: &str, tool_input: &Value) -> ClassifierVerdict {
        let body = json!({
            "model": self.model,
            "max_tokens": 16,
            "system": SYSTEM_PROMPT,
            "messages": [{ "role": "user", "content": build_classifier_input(tool_name, tool_input) }],
        });
        let mut req = self
            .http
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("anthropic-version", "2023-06-01");
        for (k, v) in &self.extra_headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = match req.json(&body).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return ClassifierVerdict::Prompt, // fail safe
        };
        let Ok(value) = resp.json::<Value>().await else {
            return ClassifierVerdict::Prompt;
        };
        parse_verdict(&extract_text(&value))
    }
}

/// Build the shared `auto`-mode LLM safety classifier used by all Trogon runners.
///
/// Reads `AUTO_CLASSIFIER_MODEL` (default: `claude-haiku-4-5-20251001`). Uses the
/// same proxy + Anthropic credentials as the ACP runner's agent loop.
pub fn build_auto_safety_classifier(
    http: reqwest::Client,
    proxy_url: &str,
    anthropic_base_url: Option<&str>,
    anthropic_token: impl Into<String>,
) -> Arc<dyn SafetyClassifier> {
    let model = std::env::var("AUTO_CLASSIFIER_MODEL")
        .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
    Arc::new(LlmSafetyClassifier::new(
        http,
        proxy_url,
        anthropic_base_url,
        anthropic_token,
        model,
        vec![],
    ))
}

impl SafetyClassifier for LlmSafetyClassifier {
    fn classify<'a>(
        &'a self,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ClassifierVerdict> + Send + 'a>> {
        Box::pin(self.do_classify(tool_name, tool_input))
    }
}

/// Build the user message describing a tool call for the classifier.
fn build_classifier_input(tool_name: &str, tool_input: &Value) -> String {
    let mut rendered = tool_input.to_string();
    if rendered.len() > MAX_INPUT_CHARS {
        rendered.truncate(MAX_INPUT_CHARS);
        rendered.push('…');
    }
    format!("Tool: {tool_name}\nArguments (JSON): {rendered}\n\nVerdict (ALLOW/DENY/PROMPT):")
}

/// Concatenate the text blocks of an Anthropic Messages API response.
fn extract_text(resp: &Value) -> String {
    resp.get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// Map a model reply to a verdict. DENY wins over ALLOW (conservative); anything
/// else — including an empty/garbled reply — is PROMPT (fail safe).
fn parse_verdict(text: &str) -> ClassifierVerdict {
    let upper = text.trim().to_ascii_uppercase();
    // Refusal phrasings that embed "ALLOW" must be checked before a plain ALLOW match.
    if upper.contains("DENY")
        || upper.contains("DISALLOW")
        || upper.contains("NOT ALLOWED")
        || upper.contains("NOT ALLOW")
    {
        ClassifierVerdict::Deny
    } else if upper
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| tok == "ALLOW")
    {
        ClassifierVerdict::Allow
    } else {
        ClassifierVerdict::Prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn parse_verdict_maps_keywords_and_defaults_to_prompt() {
        assert_eq!(parse_verdict("ALLOW"), ClassifierVerdict::Allow);
        assert_eq!(parse_verdict("allow"), ClassifierVerdict::Allow);
        assert_eq!(parse_verdict("DENY"), ClassifierVerdict::Deny);
        assert_eq!(parse_verdict("PROMPT"), ClassifierVerdict::Prompt);
        assert_eq!(parse_verdict(""), ClassifierVerdict::Prompt);
        assert_eq!(parse_verdict("I'm not sure"), ClassifierVerdict::Prompt);
        // DENY is conservative — wins if both somehow appear.
        assert_eq!(parse_verdict("ALLOW? no, DENY"), ClassifierVerdict::Deny);
    }

    #[test]
    fn parse_verdict_rejects_disallow_and_not_allowed_phrasings() {
        assert_ne!(parse_verdict("DISALLOW"), ClassifierVerdict::Allow);
        assert_eq!(parse_verdict("DISALLOW"), ClassifierVerdict::Deny);
        assert_ne!(parse_verdict("NOT ALLOWED"), ClassifierVerdict::Allow);
        assert_eq!(parse_verdict("NOT ALLOWED"), ClassifierVerdict::Deny);
        assert_eq!(parse_verdict("ALLOW"), ClassifierVerdict::Allow);
        assert_eq!(parse_verdict("DENY"), ClassifierVerdict::Deny);
        assert_eq!(parse_verdict(""), ClassifierVerdict::Prompt);
    }

    #[test]
    fn build_input_includes_tool_and_truncates() {
        let input = build_classifier_input("write_file", &json!({"path": "a.rs"}));
        assert!(input.contains("write_file"));
        assert!(input.contains("a.rs"));
        let big = json!({ "content": "x".repeat(5000) });
        assert!(build_classifier_input("t", &big).len() < 5000 + 100);
    }

    fn extract_text_value(text: &str) -> Value {
        json!({ "content": [{ "type": "text", "text": text }] })
    }

    #[test]
    fn extract_text_joins_blocks() {
        assert_eq!(extract_text(&extract_text_value("ALLOW")), "ALLOW");
        assert_eq!(extract_text(&json!({})), "");
    }

    #[tokio::test]
    async fn classify_allow_via_http() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/anthropic/v1/messages");
            then.status(200).json_body(extract_text_value("ALLOW"));
        });
        let c = LlmSafetyClassifier::new(
            reqwest::Client::new(),
            &server.base_url(),
            None,
            "tok",
            "claude-haiku",
            vec![],
        );
        let v = c
            .do_classify("write_file", &json!({"path": "a.rs", "content": "x"}))
            .await;
        assert_eq!(v, ClassifierVerdict::Allow);
    }

    #[tokio::test]
    async fn classify_http_error_fails_safe_to_prompt() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/anthropic/v1/messages");
            then.status(500);
        });
        let c = LlmSafetyClassifier::new(
            reqwest::Client::new(),
            &server.base_url(),
            None,
            "tok",
            "m",
            vec![],
        );
        assert_eq!(
            c.do_classify("bash", &json!({"command": "rm -rf /"})).await,
            ClassifierVerdict::Prompt
        );
    }

    #[tokio::test]
    async fn classify_unreachable_fails_safe_to_prompt() {
        let c = LlmSafetyClassifier::new(
            reqwest::Client::new(),
            "http://127.0.0.1:1", // nothing listening
            None,
            "tok",
            "m",
            vec![],
        );
        assert_eq!(
            c.do_classify("bash", &json!({"command": "x"})).await,
            ClassifierVerdict::Prompt
        );
    }
}
