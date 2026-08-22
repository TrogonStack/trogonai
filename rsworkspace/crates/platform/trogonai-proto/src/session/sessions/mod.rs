mod codec;
mod validate;

// Thin wrappers that re-export the generated proto packages, emitted as inline
// module trees that mirror the codegen layout.
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod artifacts_v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::artifacts::v1alpha1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod diff_v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::diff::v1alpha1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod doctor_v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::doctor::v1alpha1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod maintenance_v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::maintenance::v1alpha1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod queries_v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::queries::v1alpha1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod replay_v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::replay::v1alpha1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod state_v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::state::v1alpha1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::v1alpha1::*;
}

pub use codec::SessionEventPayloadError;
pub use v1alpha1::__buffa::oneof::session_event::Event as SessionEventCase;
pub use validate::{SessionEventValidationError, validate_session_event};
