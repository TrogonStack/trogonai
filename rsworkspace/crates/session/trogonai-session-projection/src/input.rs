use buffa_types::google::protobuf::Timestamp;
use trogonai_capabilities::ResolvedCapabilities;
use trogonai_session_contracts::{
    CanonicalMessage, ContextTwin, SessionSnapshotState, SwitchAdaptationPlan,
};

use crate::config::ProjectionConfig;

/// Compile-time inputs for a deterministic prompt projection.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionInput {
    pub session_id: String,
    pub model_id: String,
    pub snapshot: SessionSnapshotState,
    pub context_twin: ContextTwin,
    pub adaptation_plan: Option<SwitchAdaptationPlan>,
    pub capabilities: ResolvedCapabilities,
    pub token_budget: u64,
    pub current_request: Option<CanonicalMessage>,
    pub continuity_warnings: Vec<String>,
    pub config: ProjectionConfig,
    /// Fixed timestamp for deterministic tests.
    pub created_at: Option<Timestamp>,
    /// Fixed projection id for deterministic tests.
    pub projection_id: Option<String>,
}

impl ProjectionInput {
    pub fn token_budget_for_model(&self) -> u64 {
        self.token_budget
            .min(self.capabilities.schema.max_context_tokens)
    }
}
