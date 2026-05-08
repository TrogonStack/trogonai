use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A single durable fact extracted from a session transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    /// Broad category: `preference`, `constraint`, `goal`, `fact`, or `pattern`.
    pub category: String,
    /// The fact itself, in plain language.
    pub content: String,
    /// Model's confidence that this fact is accurate and relevant (0.0–1.0).
    pub confidence: f32,
    /// Session that produced this fact.
    pub source_session: String,
    /// Wall-clock time this fact was extracted (ms since Unix epoch).
    pub timestamp: u64,
}

/// Accumulated cross-session memory for one entity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityMemory {
    /// All facts extracted across all sessions for this entity.
    pub facts: Vec<MemoryFact>,
    /// Wall-clock time the memory was last updated (ms since Unix epoch).
    pub updated_at: u64,
}

impl EntityMemory {
    /// Append `new_facts` (tagging them with `session_id` and current time)
    /// and bump `updated_at`.
    pub fn merge(&mut self, new_facts: Vec<RawFact>, session_id: &str) {
        let now = now_ms();
        for raw in new_facts {
            self.facts.push(MemoryFact {
                category: raw.category,
                content: raw.content,
                confidence: raw.confidence,
                source_session: session_id.to_string(),
                timestamp: now,
            });
        }
        self.updated_at = now;
    }
}

/// Fact as returned by the LLM before session/timestamp are applied.
#[derive(Debug, Clone, Deserialize)]
pub struct RawFact {
    pub category: String,
    pub content: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    0.8
}

/// Payload published to `sessions.dream.>` to trigger memory extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamTrigger {
    pub actor_type: String,
    pub actor_key: String,
    pub session_id: String,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DreamerError {
    Llm(String),
    Parse(String),
    Store(String),
    Transcript(String),
}

impl std::fmt::Display for DreamerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DreamerError::Llm(e) => write!(f, "LLM call failed: {e}"),
            DreamerError::Parse(e) => write!(f, "failed to parse LLM response: {e}"),
            DreamerError::Store(e) => write!(f, "store error: {e}"),
            DreamerError::Transcript(e) => write!(f, "transcript error: {e}"),
        }
    }
}

impl std::error::Error for DreamerError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_tags_facts_with_session_and_timestamp() {
        let mut memory = EntityMemory::default();
        let raw = vec![
            RawFact {
                category: "preference".into(),
                content: "prefers dark mode".into(),
                confidence: 0.9,
            },
            RawFact {
                category: "goal".into(),
                content: "wants to ship by Friday".into(),
                confidence: 0.8,
            },
        ];
        memory.merge(raw, "sess-abc");
        assert_eq!(memory.facts.len(), 2);
        assert!(memory.facts.iter().all(|f| f.source_session == "sess-abc"));
        assert!(memory.facts.iter().all(|f| f.timestamp > 0));
        assert!(memory.updated_at > 0);
    }

    #[test]
    fn merge_accumulates_across_calls() {
        let mut memory = EntityMemory::default();
        memory.merge(
            vec![RawFact { category: "fact".into(), content: "a".into(), confidence: 1.0 }],
            "sess-1",
        );
        memory.merge(
            vec![RawFact { category: "fact".into(), content: "b".into(), confidence: 1.0 }],
            "sess-2",
        );
        assert_eq!(memory.facts.len(), 2);
        assert_eq!(memory.facts[0].source_session, "sess-1");
        assert_eq!(memory.facts[1].source_session, "sess-2");
    }

    #[test]
    fn raw_fact_uses_default_confidence_when_absent() {
        let json = r#"{"category":"fact","content":"x"}"#;
        let raw: RawFact = serde_json::from_str(json).unwrap();
        assert!((raw.confidence - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn dream_trigger_round_trips() {
        let trigger = DreamTrigger {
            actor_type: "pr".into(),
            actor_key: "owner/repo/456".into(),
            session_id: "sess-xyz".into(),
        };
        let json = serde_json::to_string(&trigger).unwrap();
        let parsed: DreamTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.actor_type, "pr");
        assert_eq!(parsed.actor_key, "owner/repo/456");
        assert_eq!(parsed.session_id, "sess-xyz");
    }
}
