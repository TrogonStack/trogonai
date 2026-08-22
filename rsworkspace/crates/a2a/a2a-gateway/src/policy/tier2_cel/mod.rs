#![cfg_attr(
    dylint_lib = "trogon_lints",
    expect(
        acyclic_modules,
        reason = "a compiled bundle holds the programs the compiler produced and the compiler is typed with the handles the bundle hands back"
    )
)]

pub mod bundle;
pub mod compiler;
pub mod evaluator;

#[cfg(test)]
mod tests;

pub use bundle::{CelProgramHandle, Tier2CompiledBundle};
pub use compiler::CelCompileError;
pub use evaluator::{CelEngine, CelInterpreterEngine, RealTier2CelEvaluator, tier2_evaluation_context_from_ingress};
