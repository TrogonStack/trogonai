// Thin wrapper that re-exports the generated proto package, emitted as an
// inline module tree that mirrors the codegen layout.
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod v1 {
    pub use crate::r#gen::trogonai::decider::v1::*;
}

pub use v1::__buffa::oneof::command_faulted::Kind as CommandFaultedKind;
pub use v1::__buffa::oneof::command_outcome::Outcome as CommandOutcomeCase;
