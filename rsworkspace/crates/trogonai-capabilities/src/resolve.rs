use time::OffsetDateTime;
use trogon_registry::{Registry, RegistryStore};
use trogonai_session_contracts::CapabilitySchema;

use crate::config::CapabilityConfig;
use crate::error::CapabilityError;
use crate::freshness::{FreshnessStatus, apply_freshness_policy};
use crate::registry::CapabilityRegistry;
use crate::telemetry::metrics;

/// Resolved capabilities for a model/runner pair after freshness policy application.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCapabilities {
    pub schema: CapabilitySchema,
    pub runner_id: String,
    pub freshness: FreshnessStatus,
    pub degraded: bool,
}

/// Resolve model capabilities from `AGENT_REGISTRY`, applying freshness policy.
pub async fn resolve_model_capabilities<S: RegistryStore>(
    registry: &Registry<S>,
    model_id: &str,
    now: OffsetDateTime,
    config: &CapabilityConfig,
) -> Result<ResolvedCapabilities, CapabilityError> {
    let capability_registry = CapabilityRegistry;
    let Some((agent, schema)) = capability_registry.lookup_schema(registry, model_id).await? else {
        return Err(CapabilityError::ModelNotFound {
            model_id: model_id.to_string(),
        });
    };

    let runner_id = schema.runner_id.clone();
    if runner_id.is_empty() {
        return Err(CapabilityError::SchemaMissing {
            model_id: model_id.to_string(),
            runner_id: agent.agent_type.clone(),
        });
    }

    let (schema, freshness, degraded) = apply_freshness_policy(schema, now, config);
    metrics::record_capability_resolved(
        model_id,
        &runner_id,
        freshness_label(freshness),
        degraded,
        schema.confidence,
    );

    Ok(ResolvedCapabilities {
        schema,
        runner_id,
        freshness,
        degraded,
    })
}

fn freshness_label(status: FreshnessStatus) -> &'static str {
    match status {
        FreshnessStatus::Fresh => "fresh",
        FreshnessStatus::Stale => "stale",
        FreshnessStatus::Unverified => "unverified",
    }
}
