//! Fixtures shared by the integration tests. Each test binary uses a subset.
#![allow(dead_code, clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use trogon_policy_eval::{
    Action, AgentId, Callee, Chain, DelegationPermission, DelegationTarget, EgressDestination, EgressHostName,
    EgressPermission, EnforcementMode, Evaluator, Hold, HoldName, HoldScope, InMemoryPolicyView, ModelCeiling, ModelId,
    NodeId, Phase, PolicySnapshot, PolicySnapshotBuilder, Port, Request, RuleKind, RuleRef, StalenessBudget,
    StreamRevision, Subject,
};

pub fn agent_id(value: &str) -> AgentId {
    AgentId::new(value).expect("valid agent id")
}

pub fn node_id(value: &str) -> NodeId {
    NodeId::new(value).expect("valid node id")
}

pub fn agent(value: &str) -> Subject {
    Subject::Agent(agent_id(value))
}

pub fn node(value: &str) -> Subject {
    Subject::Node(node_id(value))
}

pub fn chain(subjects: Vec<Subject>) -> Chain {
    Chain::new(subjects).expect("non-empty chain")
}

/// The chain of agent `a`: the agent, then the `team` and `org` nodes above it.
pub fn chain_of_a() -> Chain {
    chain(vec![agent("a"), node("team"), node("org")])
}

pub fn host(value: &str) -> EgressHostName {
    EgressHostName::new(value).expect("valid host")
}

pub fn port(value: u16) -> Port {
    Port::new(value).expect("valid port")
}

pub fn model(value: &str) -> ModelId {
    ModelId::new(value).expect("valid model id")
}

pub fn hold(name: &str, scope: HoldScope) -> Hold {
    Hold::new(HoldName::new(name).expect("valid hold name"), scope)
}

pub fn destination(host_name: &str, port_number: u16) -> EgressDestination {
    EgressDestination::new(host(host_name), port(port_number))
}

pub fn egress_permission(destinations: &[(&str, u16)]) -> EgressPermission {
    destinations.iter().map(|(h, p)| destination(h, *p)).collect()
}

pub fn delegation_permission(targets: Vec<DelegationTarget>) -> DelegationPermission {
    targets.into_iter().collect()
}

pub fn model_ceiling(models: &[&str]) -> ModelCeiling {
    models.iter().map(|m| model(m)).collect()
}

/// The instant every fixture snapshot is stamped with.
pub fn stamped_at() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000)
}

/// One second after the stamp, well inside any budget.
pub fn fresh_now() -> SystemTime {
    stamped_at() + Duration::from_secs(1)
}

pub fn builder() -> PolicySnapshotBuilder {
    PolicySnapshot::builder(stamped_at())
}

pub fn empty_snapshot() -> PolicySnapshot {
    builder().build()
}

pub fn view(snapshot: PolicySnapshot) -> InMemoryPolicyView {
    InMemoryPolicyView::new(snapshot)
}

pub fn enforce() -> Evaluator {
    Evaluator::new(EnforcementMode::Enforce, StalenessBudget::default())
}

pub fn dry_run() -> Evaluator {
    Evaluator::new(EnforcementMode::DryRun, StalenessBudget::default())
}

pub fn egress(own_chain: Chain, host_name: &str, port_number: u16) -> Request {
    Request::without_delegation(
        own_chain,
        Action::EgressConnect {
            host: host(host_name),
            port: port(port_number),
        },
        Phase::ProtectedAction,
    )
}

pub fn model_use(own_chain: Chain, lineage: Chain, model_name: &str, phase: Phase) -> Request {
    Request::new(
        own_chain,
        lineage,
        Action::ModelUse {
            model: model(model_name),
        },
        phase,
    )
    .expect("lineage holds the own chain")
}

pub fn delegate(own_chain: Chain, lineage: Chain, callee: Callee, phase: Phase) -> Request {
    Request::new(own_chain, lineage, Action::Delegate { callee }, phase).expect("lineage holds the own chain")
}

pub fn rule_ref(kind: RuleKind, subject: Subject, revision: u64) -> RuleRef {
    RuleRef::new(kind, subject, StreamRevision::new(revision))
}
