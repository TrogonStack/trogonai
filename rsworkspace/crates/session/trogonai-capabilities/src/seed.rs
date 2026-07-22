//! Kernel-owned baseline capability schemas for the initial certified providers.
//!
//! The design (`cambio-modelo.md`, Component 14 "Model capabilities" and the
//! "Open Implementation Decisions": *"Session Kernel owns schemas; runners solo
//! adaptan/proyectan"*, plus the backlog *"Matriz inicial con dos proveedores/
//! runners y capabilities esperadas"*) makes the core the owner of model
//! capability schemas.
//!
//! Until runners self-register verified schemas — or probing/certification
//! populates `AGENT_REGISTRY` — [`resolve_model_capabilities`](crate::resolve)
//! falls back to this manually-curated baseline for the certified provider set so
//! the canonical switch path can negotiate capabilities instead of failing with
//! `ModelNotFound`. Models outside the set return `None`, so a switch to an
//! uncharacterized model is still refused by the Safety Gate rather than guessed.
//!
//! The values mirror the project's existing Claude/Grok reference schemas and are
//! tagged `CapabilitySource::Manual` (below a verified probe/registry schema, but
//! above the default freshness `min_confidence`). `last_verified_at` is stamped at
//! resolution time so the manual baseline reads as fresh.

use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use time::OffsetDateTime;
use trogonai_session_contracts::{CapabilitySchema, CapabilitySource, SCHEMA_VERSION_V1, ToolResultFormat};

/// Confidence for the manually-curated baseline. Intentionally below a verified
/// registry/probe schema (`0.9`+) but above the default freshness `min_confidence`
/// so the baseline is treated as usable rather than degraded to conservative.
const BASELINE_CONFIDENCE: f64 = 0.8;
/// Baseline schemas are revalidated daily, matching the registry default TTL.
const BASELINE_TTL_SECONDS: u64 = 86_400;

/// Kernel-owned baseline capability schema for a model in the initial certified
/// set, or `None` for models outside it.
///
/// `now` stamps `last_verified_at` so the manual baseline is fresh at resolution
/// time. Matching is family-based (substring) so model revisions within a family
/// share the same baseline (e.g. any `claude-*` id).
pub fn known_model_schema(model_id: &str, now: OffsetDateTime) -> Option<CapabilitySchema> {
    let lower = model_id.to_ascii_lowercase();

    let base = |runner_id: &str| CapabilitySchema {
        schema_version: SCHEMA_VERSION_V1,
        model_id: model_id.to_string(),
        runner_id: runner_id.to_string(),
        tool_result_format: EnumValue::Known(ToolResultFormat::Json),
        source: EnumValue::Known(CapabilitySource::Manual),
        last_verified_at: MessageField::some(Timestamp {
            seconds: now.unix_timestamp(),
            nanos: 0,
            ..Timestamp::default()
        }),
        ttl_seconds: BASELINE_TTL_SECONDS,
        confidence: BASELINE_CONFIDENCE,
        ..CapabilitySchema::default()
    };

    if lower.contains("claude") {
        // Anthropic Claude family (served by `trogon-acp-runner`, whose
        // `AGENT_TYPE` defaults to "claude").
        Some(CapabilitySchema {
            max_context_tokens: 200_000,
            max_output_tokens: 8_192,
            tool_use: true,
            parallel_tool_calls: true,
            image_input: true,
            file_input: true,
            structured_output: true,
            json_schema: true,
            reasoning: true,
            streaming: true,
            system_prompt_support: true,
            compaction_supported: true,
            ..base("claude")
        })
    } else if lower.contains("grok") {
        // xAI Grok family (served by `trogon-xai-runner`); no image input.
        Some(CapabilitySchema {
            max_context_tokens: 131_072,
            max_output_tokens: 16_384,
            tool_use: true,
            parallel_tool_calls: true,
            image_input: false,
            file_input: true,
            structured_output: true,
            json_schema: true,
            reasoning: false,
            streaming: true,
            system_prompt_support: true,
            compaction_supported: true,
            ..base("xai")
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    #[test]
    fn claude_family_resolves_with_vision_and_reasoning() {
        let schema = known_model_schema("anthropic/claude-sonnet-4-5", now()).unwrap();
        assert_eq!(schema.runner_id, "claude");
        assert!(schema.tool_use);
        assert!(schema.image_input);
        assert!(schema.reasoning);
        assert_eq!(schema.max_context_tokens, 200_000);
        assert!(matches!(schema.source.as_known(), Some(CapabilitySource::Manual)));
        // Stamped fresh at resolution time.
        assert_eq!(
            schema.last_verified_at.as_option().unwrap().seconds,
            now().unix_timestamp()
        );
    }

    #[test]
    fn grok_family_resolves_without_image_or_reasoning() {
        let schema = known_model_schema("xai/grok-code-fast", now()).unwrap();
        assert_eq!(schema.runner_id, "xai");
        assert!(schema.tool_use);
        assert!(!schema.image_input);
        assert!(!schema.reasoning);
        assert_eq!(schema.max_context_tokens, 131_072);
    }

    #[test]
    fn unknown_model_is_not_in_the_baseline_set() {
        assert!(known_model_schema("mystery/model-x", now()).is_none());
    }
}
