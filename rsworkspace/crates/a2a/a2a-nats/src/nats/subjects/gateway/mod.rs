//! Gateway subjects: `{prefix}.v1.gateway.{agent_id}.{kind}.{id}.{method…}` ingress and
//! the caller-scoped `{prefix}.v1.gateway.{kind}.{id}.events` egress fanout, where
//! `{kind}.{id}` is the caller's principal position (ADR#0082).

pub mod egress_events;
pub mod ingress;

pub use egress_events::GatewayEgressEventsSubject;
pub use ingress::{GatewayIngressError, GatewayIngressSubject};
