#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

// The generated protobuf tree under `gen/` is emitted as a single file with
// nested inline modules by `buffa-codegen`; the policy cannot apply to code
// we do not author, so the allow is scoped to that subtree.
#[allow(clippy::all)]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
#[path = "gen/mod.rs"]
#[cfg(feature = "schedules")]
mod r#gen;

#[cfg(feature = "schedules")]
mod codec;

#[cfg(feature = "chrono")]
pub mod convert;

#[cfg(feature = "schedules")]
pub mod scheduler;

#[cfg(feature = "schedules")]
pub mod content;

#[cfg(feature = "schedules")]
pub mod google;
