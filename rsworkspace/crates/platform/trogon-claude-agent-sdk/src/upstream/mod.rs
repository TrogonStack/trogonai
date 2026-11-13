//! Serde types generated from the pinned upstream SDK declarations.
//!
//! The files below are produced by `mise run claude-sdk:types` from the exact upstream
//! release recorded in `.config/mise/lib/claude-sdk-freshness/baseline.json`. They mirror
//! the upstream wire shape: open objects keep unknown keys, string-literal unions keep
//! unlisted values, and nothing is validated.

#[allow(
    dead_code,
    reason = "Helpers are referenced only by the generated fields that need them."
)]
mod support;

pub use support::KeyOrder;

#[rustfmt::skip]
#[allow(
    dead_code,
    missing_docs,
    clippy::enum_variant_names,
    clippy::large_enum_variant,
    reason = "Generated code mirrors upstream names and keeps a private serde mirror of each struct."
)]
pub mod tool_inputs;

#[rustfmt::skip]
#[allow(
    dead_code,
    missing_docs,
    clippy::enum_variant_names,
    clippy::large_enum_variant,
    reason = "Generated code mirrors upstream names and keeps a private serde mirror of each struct."
)]
pub mod tool_outputs;

#[rustfmt::skip]
#[allow(
    dead_code,
    missing_docs,
    clippy::enum_variant_names,
    clippy::large_enum_variant,
    reason = "Generated code mirrors upstream names and keeps a private serde mirror of each struct."
)]
pub mod settings;

#[rustfmt::skip]
#[allow(
    dead_code,
    missing_docs,
    clippy::enum_variant_names,
    clippy::large_enum_variant,
    reason = "Generated code mirrors upstream names and keeps a private serde mirror of each struct."
)]
pub mod settings_resolution;

#[rustfmt::skip]
#[allow(
    dead_code,
    missing_docs,
    clippy::enum_variant_names,
    clippy::large_enum_variant,
    reason = "Generated code mirrors upstream names and keeps a private serde mirror of each struct."
)]
pub mod messages;

#[rustfmt::skip]
#[allow(
    dead_code,
    missing_docs,
    clippy::enum_variant_names,
    clippy::large_enum_variant,
    reason = "Generated code mirrors upstream names and keeps a private serde mirror of each struct."
)]
pub mod control;
