//! `TryFrom` conversions from the generated `policy` messages into the value
//! objects of this crate.
//!
//! The generated messages are unvalidated input: the `buf.validate`
//! annotations in the schema document constraints and nothing enforces them
//! (ADR#0064). Every conversion here goes through the constructor of the
//! domain type, so a value that comes out is valid for good and a value that
//! is not surfaces as a [`ProtoConversionError`].

mod agent_id;
mod delegation_permission;
mod delegation_target;
mod egress_destination;
mod egress_permission;
mod hold;
mod hold_name;
mod hold_scope;
mod model_ceiling;
mod model_id;
mod node_id;
mod proto_conversion_error;
mod rule_kind;
mod rule_ref;
mod subject;

pub use proto_conversion_error::ProtoConversionError;
