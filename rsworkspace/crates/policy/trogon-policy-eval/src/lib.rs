//! Pure, synchronous evaluation of policy bindings.
//!
//! This crate implements the evaluation rules of
//! [ADR#0071](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md):
//!
//! - An action is admitted when some permit on the dispatching Agent's own chain
//!   covers it, every ceiling on the delegation lineage admits it, and no hold
//!   on the lineage refuses it.
//! - Absence of a permit is denial. Absence of a ceiling is no ceiling, never a
//!   zero cap.
//! - There is no precedence and there are no deny lists, so each refusal names
//!   exactly one binding.
//! - Request rules (CEL, declarative, Wasm, guardrails) can only narrow. They
//!   reach this crate as [`Narrowing`] results through [`RequestRuleEngine`],
//!   and [`Narrowing`] has no arm that permits.
//! - Live means a bounded staleness. A [`PolicySnapshot`] older than the
//!   [`StalenessBudget`], at most 330 seconds, makes the evaluator refuse.
//!
//! The evaluator reads a [`PolicySnapshot`] handed out by a [`PolicyView`] and
//! takes the current time from the caller. It performs no I/O and has no async
//! surface.
//!
//! # Features
//!
//! `decider` adds `PolicyAuthorizer`, which plugs the evaluator into the
//! decider runtime's `CommandAuthorizer` seam.
//!
//! `proto` adds `TryFrom` conversions from the generated `policy` messages
//! (subjects, rule refs, holds, model ceilings, and the egress and delegation
//! permissions) into the types of this crate, failing with
//! [`ProtoConversionError`]. A projection that reads decider state uses them
//! to fill a [`PolicySnapshotBuilder`].
//!
//! The core compiles and tests without either.
//!
//! # Not converted
//!
//! Policy state wraps a history in a `Missing` or `Holds` arm and carries the
//! stream revision outside the message. The snapshot represents a missing
//! history by leaving it out, so the projection owns that step and converts
//! the messages inside it.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod constants;

mod action;
mod agent_id;
mod callee;
mod chain;
mod decision;
mod delegation_permission;
mod delegation_target;
mod effective;
mod egress_destination;
mod egress_host_name;
mod egress_permission;
mod enforcement_mode;
mod evaluator;
mod evaluator_error;
mod hold;
mod hold_name;
mod hold_scope;
mod identifier;
mod in_memory_policy_view;
mod model_ceiling;
mod model_id;
mod narrowing;
mod node_id;
mod phase;
mod port;
mod refusal;
mod request;
mod request_rule_engine;
mod rule_kind;
mod rule_name;
mod rule_ref;
mod snapshot;
mod staleness_budget;
mod stream_revision;
mod subject;
mod view;

#[cfg(feature = "proto")]
mod proto;

#[cfg(feature = "decider")]
mod action_mapper;
#[cfg(feature = "decider")]
mod authorizer;
#[cfg(feature = "decider")]
mod decision_recorder;
#[cfg(feature = "decider")]
mod mapping_error;

pub use action::Action;
pub use agent_id::AgentId;
pub use callee::Callee;
pub use chain::{Chain, ChainError, ChainIter};
pub use decision::Decision;
pub use delegation_permission::DelegationPermission;
pub use delegation_target::DelegationTarget;
pub use effective::Effective;
pub use egress_destination::EgressDestination;
pub use egress_host_name::{EgressHostName, EgressHostNameError};
pub use egress_permission::EgressPermission;
pub use enforcement_mode::EnforcementMode;
pub use evaluator::Evaluator;
pub use evaluator_error::EvaluatorError;
pub use hold::Hold;
pub use hold_name::HoldName;
pub use hold_scope::HoldScope;
pub use identifier::IdentifierError;
pub use in_memory_policy_view::InMemoryPolicyView;
pub use model_ceiling::ModelCeiling;
pub use model_id::ModelId;
pub use narrowing::Narrowing;
pub use node_id::NodeId;
pub use phase::Phase;
pub use port::{Port, PortError};
pub use refusal::Refusal;
pub use request::{Request, RequestError};
pub use request_rule_engine::RequestRuleEngine;
pub use rule_kind::RuleKind;
pub use rule_name::{RuleName, RuleNameError};
pub use rule_ref::RuleRef;
pub use snapshot::{PolicySnapshot, PolicySnapshotBuilder};
pub use staleness_budget::{StalenessBudget, StalenessBudgetError};
pub use stream_revision::StreamRevision;
pub use subject::Subject;
pub use view::PolicyView;

#[cfg(feature = "proto")]
pub use proto::ProtoConversionError;

#[cfg(feature = "decider")]
pub use action_mapper::{ActionMapper, agent_of};
#[cfg(feature = "decider")]
pub use authorizer::PolicyAuthorizer;
#[cfg(feature = "decider")]
pub use decision_recorder::DecisionRecorder;
#[cfg(feature = "decider")]
pub use mapping_error::MappingError;
