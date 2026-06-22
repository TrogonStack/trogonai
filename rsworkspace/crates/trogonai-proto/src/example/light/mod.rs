pub mod codec;

#[cfg(feature = "light")]
mod decider;

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod state_v1 {
    pub use crate::r#gen::trogonai::example::light::state::v1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod v1 {
    pub use crate::r#gen::trogonai::example::light::v1::*;
}

pub use codec::LightEventPayloadError;
pub use v1::__buffa::oneof::light_event::Event as LightEventCase;

/// Stable type URL for the [`v1::TurnOn`] command envelope.
pub const TURN_ON_TYPE_URL: &str = v1::TurnOn::TYPE_URL;

/// Stable type URL for the [`state_v1::State`] snapshot schema version tag.
pub const LIGHT_STATE_SCHEMA_VERSION: &str = state_v1::State::TYPE_URL;
