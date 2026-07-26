mod codec;
mod validate;

// Thin wrapper that re-exports the generated proto package, emitted as an inline
// module tree that mirrors the codegen layout.
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod v1alpha1 {
    pub use crate::r#gen::trogonai::session::sessions::v1alpha1::*;
}

pub use codec::SessionEventPayloadError;
pub use v1alpha1::__buffa::oneof::session_event::Event as SessionEventCase;
pub use validate::{SessionEventValidationError, validate_session_event};
