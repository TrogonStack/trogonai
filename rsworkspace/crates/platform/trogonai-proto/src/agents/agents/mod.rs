mod codec;

// Thin wrapper that re-exports the generated proto package, emitted as an
// inline module tree that mirrors the codegen layout.
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod state_v1 {
    pub use crate::r#gen::trogonai::agents::agents::state::v1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod v1 {
    pub use crate::r#gen::trogonai::agents::agents::v1::*;
}

pub use codec::AgentEventPayloadError;
pub use v1::__buffa::oneof::agent_event::Event as AgentEventCase;

/// Stable type URLs for agent command envelopes.
pub const PROVISION_AGENT_TYPE_URL: &str = v1::ProvisionAgent::TYPE_URL;

/// Schema-version tag for a [`state_v1::ProvisionAgentState`] snapshot.
pub const PROVISION_AGENT_STATE_SCHEMA_VERSION: &str = <state_v1::ProvisionAgentState as buffa::MessageName>::FULL_NAME;
