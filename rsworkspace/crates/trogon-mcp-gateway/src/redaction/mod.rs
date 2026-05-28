//! Schema-driven field redaction before audit, anomaly, and low-trust egress paths.

mod engine;
mod errors;
mod rule;
mod ruleset;

pub use engine::redact;
pub use errors::RedactionError;
pub use rule::{JsonPath, RedactionAction, RedactionRule};
pub use ruleset::RedactionRuleset;
