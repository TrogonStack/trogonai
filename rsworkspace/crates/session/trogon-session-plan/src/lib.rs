use trogonai_proto::session_contracts::trogonai::{agents::agents::v1alpha1 as agent, session::{plans::v1alpha1 as plan, sessions::v1alpha1 as session}};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("session plan contract is unsupported")]
    UnsupportedContract,
}

pub struct RegisteredPlanDecoder {
    contract: agent::ConfigurationContractRef,
}

pub struct DecodedPlan {
    value: plan::SessionExecutionPlan,
    original: Vec<u8>,
}

impl DecodedPlan {
    pub fn value(&self) -> &plan::SessionExecutionPlan { &self.value }
    pub fn original_bytes(&self) -> &[u8] { &self.original }
}

impl RegisteredPlanDecoder {
    pub fn new(contract: agent::ConfigurationContractRef) -> Result<Self, PlanError> {
        Ok(Self { contract })
    }

    pub fn decode(&self, _stored: &session::StoredSessionExecutionPlan) -> Result<DecodedPlan, PlanError> {
        let _ = &self.contract;
        Err(PlanError::UnsupportedContract)
    }
}
