use buffa::{Message, MessageName};
use sha2::{Digest as _, Sha256};
use trogon_proto_preflight::{WireLimits, WireMessageDescriptor, WireSchemaPool, preflight};

use crate::{
    PlanError, shape,
    wire::{agent, plan, session},
};

#[derive(Clone)]
pub struct RegisteredPlanDecoder {
    contract: agent::ConfigurationContractRef,
    schema: WireSchemaPool,
    descriptor: WireMessageDescriptor,
}

pub struct DecodedPlan {
    value: plan::SessionExecutionPlan,
    original: Vec<u8>,
    digest: [u8; 32],
}

impl std::fmt::Debug for DecodedPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecodedPlan")
            .field("encoded_bytes", &self.original.len())
            .finish_non_exhaustive()
    }
}

impl DecodedPlan {
    pub fn value(&self) -> &plan::SessionExecutionPlan {
        &self.value
    }
    pub fn original_bytes(&self) -> &[u8] {
        &self.original
    }
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

impl RegisteredPlanDecoder {
    /// The host selects this immutable tuple from its protected registration,
    /// before reading request-controlled plan bytes.
    pub fn compiled_contract() -> agent::ConfigurationContractRef {
        let profile = include_bytes!("profile_v1.json");
        let descriptor = trogonai_proto::session_contracts::PLAN_DESCRIPTOR_SET_JSON;
        let mut hash = Sha256::new();
        hash.update(b"session-plan-contract-v1\0");
        hash.update((profile.len() as u64).to_be_bytes());
        hash.update(profile);
        hash.update((descriptor.len() as u64).to_be_bytes());
        hash.update(descriptor);
        agent::ConfigurationContractRef {
            message_type: plan::SessionExecutionPlan::FULL_NAME.into(),
            version: "1".into(),
            definition_digest: agent::Digest {
                algorithm: "sha256".into(),
                value: hash.finalize().to_vec(),
            }
            .into(),
        }
    }

    pub fn new(contract: agent::ConfigurationContractRef) -> Result<Self, PlanError> {
        shape::contract(&contract)?;
        if contract != Self::compiled_contract() {
            return Err(PlanError::UnsupportedContract);
        }
        let schema =
            WireSchemaPool::from_descriptor_set_json(trogonai_proto::session_contracts::PLAN_DESCRIPTOR_SET_JSON)
                .map_err(|_| PlanError::UnsupportedContract)?;
        let descriptor = schema
            .message_by_name(&contract.message_type)
            .ok_or(PlanError::UnsupportedContract)?;
        Ok(Self {
            contract,
            schema,
            descriptor,
        })
    }

    /// Decoding establishes artifact integrity and local shape, never current
    /// resource authority or support for the plan's runtime declarations.
    pub fn decode(&self, stored: &session::StoredSessionExecutionPlan) -> Result<DecodedPlan, PlanError> {
        if stored.plan_bytes.is_empty() || stored.plan_bytes.len() > 65536 {
            return Err(PlanError::Limit);
        }
        let digest = stored.plan_digest.as_option().ok_or(PlanError::DigestMismatch)?;
        shape::digest(&digest.algorithm, &digest.value)?;
        let actual: [u8; 32] = Sha256::digest(&stored.plan_bytes).into();
        if digest.value.as_slice() != actual {
            return Err(PlanError::DigestMismatch);
        }
        preflight(&self.descriptor, &stored.plan_bytes, limits()?).map_err(|_| PlanError::Wire)?;
        let value = plan::SessionExecutionPlan::decode_from_slice(&stored.plan_bytes).map_err(|_| PlanError::Wire)?;
        if value.contract.as_option() != Some(&self.contract) || value.contract_version != Some(1) {
            return Err(PlanError::UnsupportedContract);
        }
        shape::plan(&value)?;
        Ok(DecodedPlan {
            value,
            original: stored.plan_bytes.clone(),
            digest: actual,
        })
    }

    pub fn decode_agent_configuration(&self, original: &[u8]) -> Result<agent::AgentConfiguration, PlanError> {
        if original.is_empty() || original.len() > 65536 {
            return Err(PlanError::Limit);
        }
        let descriptor = self
            .schema
            .message_by_name(agent::AgentConfiguration::FULL_NAME)
            .ok_or(PlanError::UnsupportedContract)?;
        preflight(&descriptor, original, limits()?).map_err(|_| PlanError::Wire)?;
        let configuration = agent::AgentConfiguration::decode_from_slice(original).map_err(|_| PlanError::Wire)?;
        if configuration.contract_version != 1 {
            return Err(PlanError::UnsupportedContract);
        }
        Ok(configuration)
    }
}

fn limits() -> Result<WireLimits, PlanError> {
    WireLimits::new(65536, 32, 8192).map_err(|_| PlanError::Limit)
}
