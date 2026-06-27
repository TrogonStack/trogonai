//! Bridges [`Decider`](trogon_decider::Decider) implementations to the Trogon WIT guest contract.

#![doc = "\n\
Guest deciders must use [`std::collections::BTreeMap`] (not `HashMap`) for any map fields — \
proto `map<>` decoded via buffa seeds a hasher that pulls `wasi:random` on wasip2.\n\
"]

mod bridge;
mod snapshot;

pub use bridge::{
    AnyEnvelopeParts, AnyEnvelopeView, CommandEnvelopeView, DecideErrorView, DomainErrorParts, WritePreconditionTag,
    decide_command, decode_command, evolve_one, map_write_precondition,
};
pub use snapshot::{decode_snapshot, encode_current, encode_snapshot, load_or_initial};
pub use trogon_decider_guest_macros::export_decider;
