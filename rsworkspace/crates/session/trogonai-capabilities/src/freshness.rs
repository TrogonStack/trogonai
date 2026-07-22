use buffa::EnumValue;
use buffa_types::google::protobuf::Timestamp;
use time::OffsetDateTime;
use trogonai_session_contracts::{CapabilitySchema, CapabilitySource, SCHEMA_VERSION_V1};

use crate::config::CapabilityConfig;

/// Freshness evaluation for a capability schema snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessStatus {
    Fresh,
    Stale,
    Unverified,
}

/// Apply registry freshness policy: stale or low-confidence schemas degrade to conservative defaults.
pub fn apply_freshness_policy(
    schema: CapabilitySchema,
    now: OffsetDateTime,
    config: &CapabilityConfig,
) -> (CapabilitySchema, FreshnessStatus, bool) {
    let status = evaluate_freshness(&schema, now, config);
    if status == FreshnessStatus::Fresh {
        return (schema, status, false);
    }

    let degraded = conservative_defaults(&schema, config);
    (degraded, status, true)
}

fn evaluate_freshness(schema: &CapabilitySchema, now: OffsetDateTime, config: &CapabilityConfig) -> FreshnessStatus {
    let source = schema.source.as_known();
    if matches!(source, Some(CapabilitySource::Unspecified) | None) {
        return FreshnessStatus::Unverified;
    }

    if schema.confidence > 0.0 && schema.confidence < config.min_confidence {
        return FreshnessStatus::Unverified;
    }

    let ttl = if schema.ttl_seconds > 0 {
        schema.ttl_seconds
    } else {
        config.default_ttl_secs
    };

    let Some(verified_at) = schema.last_verified_at.as_option() else {
        return FreshnessStatus::Unverified;
    };

    let verified = timestamp_to_offset(verified_at);
    let age_secs = (now - verified).whole_seconds().max(0) as u64;
    if age_secs > ttl {
        FreshnessStatus::Stale
    } else {
        FreshnessStatus::Fresh
    }
}

fn conservative_defaults(schema: &CapabilitySchema, config: &CapabilityConfig) -> CapabilitySchema {
    CapabilitySchema {
        schema_version: if schema.schema_version == 0 {
            SCHEMA_VERSION_V1
        } else {
            schema.schema_version
        },
        model_id: schema.model_id.clone(),
        runner_id: schema.runner_id.clone(),
        max_context_tokens: config.conservative_max_context_tokens,
        max_output_tokens: config.conservative_max_output_tokens,
        tool_use: false,
        parallel_tool_calls: false,
        image_input: false,
        file_input: false,
        structured_output: false,
        json_schema: false,
        reasoning: false,
        streaming: schema.streaming,
        tool_result_format: schema.tool_result_format,
        system_prompt_support: schema.system_prompt_support,
        provider_restrictions: schema.provider_restrictions.clone(),
        compaction_supported: false,
        source: EnumValue::Known(CapabilitySource::Registry),
        last_verified_at: schema.last_verified_at.clone(),
        ttl_seconds: schema.ttl_seconds,
        confidence: 0.0,
        test_results: schema.test_results.clone(),
        ..CapabilitySchema::default()
    }
}

fn timestamp_to_offset(timestamp: &Timestamp) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(timestamp.seconds).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::MessageField;

    fn schema_with_verified(seconds: i64, ttl: u64, confidence: f64) -> CapabilitySchema {
        CapabilitySchema {
            schema_version: SCHEMA_VERSION_V1,
            model_id: "test-model".to_string(),
            runner_id: "test-runner".to_string(),
            max_context_tokens: 200_000,
            max_output_tokens: 8_192,
            tool_use: true,
            image_input: true,
            json_schema: true,
            source: EnumValue::Known(CapabilitySource::Probe),
            last_verified_at: MessageField::some(Timestamp {
                seconds,
                nanos: 0,
                ..Timestamp::default()
            }),
            ttl_seconds: ttl,
            confidence,
            ..CapabilitySchema::default()
        }
    }

    #[test]
    fn fresh_schema_is_not_degraded() {
        let config = CapabilityConfig::default();
        let now = OffsetDateTime::from_unix_timestamp(1_000).unwrap();
        let schema = schema_with_verified(900, 200, 0.9);
        let (resolved, status, degraded) = apply_freshness_policy(schema, now, &config);
        assert_eq!(status, FreshnessStatus::Fresh);
        assert!(!degraded);
        assert!(resolved.tool_use);
    }

    #[test]
    fn stale_schema_degrades_to_conservative_defaults() {
        let config = CapabilityConfig::default();
        let now = OffsetDateTime::from_unix_timestamp(2_000).unwrap();
        let schema = schema_with_verified(1_000, 100, 0.9);
        let (resolved, status, degraded) = apply_freshness_policy(schema, now, &config);
        assert_eq!(status, FreshnessStatus::Stale);
        assert!(degraded);
        assert!(!resolved.tool_use);
        assert!(!resolved.image_input);
        assert_eq!(resolved.max_context_tokens, config.conservative_max_context_tokens);
    }
}
