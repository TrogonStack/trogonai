//! Schema-driven field redaction before audit, anomaly, and low-trust egress paths.

mod apply;
mod engine;
mod errors;
mod outcome;
mod path;
mod registry;
mod rule;
mod ruleset;
mod schema_validate;

pub use apply::{SchemaRedactionContext, apply_schema_redaction, merge_outcomes};
pub use engine::{redact, redact_with_options, RedactionOptions};
pub use errors::RedactionError;
pub use outcome::{RedactionApplyResult, RedactionOutcome, RewriteEntry};
pub use registry::{RedactionDirection, RedactionRegistry};
pub use rule::{JsonPath, RedactionAction, RedactionRule};
pub use ruleset::RedactionRuleset;
