use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use buffa::Message as _;
use serde_json::{Map, Value};
use trogon_registry::{AgentCapability, Registry, RegistryStore};
use trogonai_session_contracts::{CapabilitySchema, SCHEMA_VERSION_V1};

use crate::error::CapabilityError;

/// Metadata key used to extend `AGENT_REGISTRY` entries with typed capability schemas.
pub const METADATA_CAPABILITY_SCHEMAS: &str = "capability_schemas";

/// Registry extension for typed model/runner capability schemas.
#[derive(Debug, Clone, Copy, Default)]
pub struct CapabilityRegistry;

impl CapabilityRegistry {
    /// Attach or update a protobuf capability schema on an agent registry entry.
    pub fn embed_schema(agent: &mut AgentCapability, schema: &CapabilitySchema) -> Result<(), CapabilityError> {
        let encoded = STANDARD.encode(schema.encode_to_vec());
        let metadata = ensure_object_metadata(&mut agent.metadata);
        let schemas = metadata
            .entry(METADATA_CAPABILITY_SCHEMAS.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(map) = schemas.as_object_mut() else {
            return Err(CapabilityError::SchemaDecode {
                model_id: schema.model_id.clone(),
                detail: "metadata.capability_schemas must be an object".to_string(),
            });
        };
        map.insert(schema.model_id.clone(), Value::String(encoded));
        Ok(())
    }

    /// Read a capability schema from an agent registry entry.
    pub fn schema_from_agent(
        agent: &AgentCapability,
        model_id: &str,
    ) -> Result<Option<CapabilitySchema>, CapabilityError> {
        let Some(encoded) = agent
            .metadata
            .get(METADATA_CAPABILITY_SCHEMAS)
            .and_then(|value| value.get(model_id))
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };

        let bytes = STANDARD.decode(encoded).map_err(|err| CapabilityError::SchemaDecode {
            model_id: model_id.to_string(),
            detail: err.to_string(),
        })?;
        let schema = CapabilitySchema::decode_from_slice(&bytes).map_err(|err| CapabilityError::SchemaDecode {
            model_id: model_id.to_string(),
            detail: err.to_string(),
        })?;
        Ok(Some(schema))
    }

    /// Register or refresh a capability schema for a runner/model pair in `AGENT_REGISTRY`.
    pub async fn register_schema<S: RegistryStore>(
        &self,
        registry: &Registry<S>,
        runner_id: &str,
        schema: CapabilitySchema,
    ) -> Result<(), CapabilityError> {
        let mut agent = registry
            .get(runner_id)
            .await?
            .ok_or_else(|| CapabilityError::SchemaMissing {
                model_id: schema.model_id.clone(),
                runner_id: runner_id.to_string(),
            })?;
        Self::embed_schema(&mut agent, &schema)?;
        registry.register(&agent).await?;
        Ok(())
    }

    /// Lookup a capability schema by model id via `find_by_model`.
    pub async fn lookup_schema<S: RegistryStore>(
        &self,
        registry: &Registry<S>,
        model_id: &str,
    ) -> Result<Option<(AgentCapability, CapabilitySchema)>, CapabilityError> {
        let Some(agent) = registry
            .find_by_model(model_id)
            .await
            .map_err(|err| CapabilityError::SchemaDecode {
                model_id: model_id.to_string(),
                detail: err,
            })?
        else {
            return Ok(None);
        };

        let schema = Self::schema_from_agent(&agent, model_id)?;
        Ok(schema.map(|schema| (agent, schema)))
    }
}

fn ensure_object_metadata(metadata: &mut Value) -> &mut Map<String, Value> {
    if metadata.is_null() {
        *metadata = Value::Object(Map::new());
    }
    metadata.as_object_mut().expect("metadata must be object")
}

pub fn default_schema_for_model(model_id: &str, runner_id: &str) -> CapabilitySchema {
    CapabilitySchema {
        schema_version: SCHEMA_VERSION_V1,
        model_id: model_id.to_string(),
        runner_id: runner_id.to_string(),
        ..CapabilitySchema::default()
    }
}
