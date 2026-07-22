//! Live capability probe: runs contract tests against a real runner/model over NATS and
//! reports which capabilities actually work (cambio-modelo.md § Capability Registry
//! Freshness: "health checks/probes por runner; contract tests para tool use, image
//! input, JSON schema, context limits, streaming y compaction"). The results feed
//! [`trogonai_capabilities::ProviderCertificationMatrix::certify_from_probes`], promoting a
//! model out of the unverified `Basic` baseline only for what was actually verified.
//!
//! It is `Send + Sync` because it talks to the runner via [`TrogonSession`] over
//! `async_nats::Client`, not the `!Send` ACP `Bridge`. A capability that cannot be
//! exercised over the text session interface (notably image input) stays unverified —
//! never assumed — per the design rule.

use std::path::PathBuf;

use trogonai_capabilities::{CapabilityProbeTransport, ProbeKind, ProbeResult};

use crate::session::{Session, StreamEvent, TrogonSession};

/// What a single probe prompt observed from the runner stream.
#[derive(Default)]
struct ProbeObservation {
    text: String,
    text_chunks: usize,
    tool_called: bool,
    context_size: u64,
}

/// [`CapabilityProbeTransport`] backed by ephemeral [`TrogonSession`]s on a target runner.
pub struct SessionCapabilityProbe {
    nats: async_nats::Client,
    prefix: String,
    model: String,
    cwd: PathBuf,
}

impl SessionCapabilityProbe {
    pub fn new(nats: async_nats::Client, prefix: String, model: String, cwd: PathBuf) -> Self {
        Self {
            nats,
            prefix,
            model,
            cwd,
        }
    }

    /// Send one prompt to the target model and collect what the stream reveals.
    async fn observe(&self, prompt: &str) -> anyhow::Result<ProbeObservation> {
        let session = TrogonSession::new(self.nats.clone(), &self.prefix, self.cwd.clone(), Vec::new()).await?;
        session.set_model(&self.model).await?;
        let mut rx = session.prompt(prompt).await?;

        let mut obs = ProbeObservation::default();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Text(chunk) => {
                    obs.text_chunks += 1;
                    obs.text.push_str(&chunk);
                }
                StreamEvent::ToolCall(_) => obs.tool_called = true,
                StreamEvent::Usage { context_size, .. } => obs.context_size = context_size,
                StreamEvent::Error(message) => {
                    let _ = session.close().await;
                    anyhow::bail!("runner error: {message}");
                }
                StreamEvent::Done(_) => break,
                _ => {}
            }
        }
        let _ = session.close().await;
        Ok(obs)
    }

    async fn probe_inner(&self, kind: ProbeKind) -> anyhow::Result<(bool, Option<String>)> {
        match kind {
            // Image input cannot be exercised over a text session — leave it unverified
            // rather than assume support ("no asumir soporte si no esta verificado").
            ProbeKind::ImageInput => Ok((false, Some("not verifiable via text session probe".to_string()))),

            // Compaction: a real round-trip via the runner's session/compact.
            ProbeKind::CompactionSupported => {
                let session =
                    TrogonSession::new(self.nats.clone(), &self.prefix, self.cwd.clone(), Vec::new()).await?;
                session.set_model(&self.model).await?;
                let result = session.compact().await;
                let _ = session.close().await;
                match result {
                    Ok(_) => Ok((true, None)),
                    Err(err) => Ok((false, Some(err.to_string()))),
                }
            }

            // Streaming: the runner delivered at least one assistant token over the stream.
            ProbeKind::Streaming => {
                let obs = self.observe("Reply with the single word: ok").await?;
                Ok((obs.text_chunks >= 1 && !obs.text.trim().is_empty(), Some(format!("chunks={}", obs.text_chunks))))
            }

            // Tool use: the model chose to call a tool when asked to.
            ProbeKind::ToolUse => {
                let obs = self.observe("Use a tool to list the files in the current directory.").await?;
                Ok((obs.tool_called, None))
            }

            // JSON schema / structured output: the model returned valid JSON matching the
            // requested shape.
            ProbeKind::JsonSchema => {
                let obs = self.observe("Reply with ONLY this JSON object and nothing else: {\"ok\": true}").await?;
                let ok = extract_json_object(&obs.text)
                    .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                    .and_then(|value| value.get("ok").and_then(|flag| flag.as_bool()))
                    .unwrap_or(false);
                Ok((ok, None))
            }

            // Context limits: the runner-reported context window meets a usable floor.
            ProbeKind::ContextLimits => {
                let obs = self.observe("Reply with: ok").await?;
                Ok((obs.context_size >= 8_192, Some(format!("context_size={}", obs.context_size))))
            }
        }
    }
}

impl CapabilityProbeTransport for SessionCapabilityProbe {
    async fn run_probe(&self, kind: ProbeKind) -> ProbeResult {
        match self.probe_inner(kind).await {
            Ok((passed, detail)) => ProbeResult { kind, passed, detail },
            Err(err) => ProbeResult {
                kind,
                passed: false,
                detail: Some(format!("probe error: {err}")),
            },
        }
    }
}

/// Extract the first balanced `{...}` span from text the model may have wrapped in prose
/// or ```json fences.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + offset + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_object_handles_fenced_and_prose() {
        assert_eq!(extract_json_object("sure: {\"ok\": true} done"), Some("{\"ok\": true}"));
        assert_eq!(
            extract_json_object("```json\n{\"a\": {\"b\": 1}}\n```"),
            Some("{\"a\": {\"b\": 1}}")
        );
        assert_eq!(extract_json_object("no json here"), None);
    }
}
