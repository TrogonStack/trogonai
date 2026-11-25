#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use common::{agent, agent_id, chain_of_a, egress_permission, hold, node, stamped_at, view};
use trogon_decider_runtime::{
    AuthorizationDeniedError, CommandAuthorizer, CommandPrincipal, PrincipalId, PrincipalKind, UnauthorizedError,
};
use trogon_policy_eval::{
    Action, Decision, DecisionRecorder, EnforcementMode, Evaluator, HoldScope, InMemoryPolicyView, MappingError, Phase,
    PolicyAuthorizer, Request, StalenessBudget, agent_of,
};
use trogon_std::time::EpochClock;

#[derive(Clone)]
struct FixedClock(SystemTime);

impl EpochClock for FixedClock {
    fn system_time(&self) -> SystemTime {
        self.0
    }
}

/// A command the application understands: connect somewhere.
struct Connect {
    host: &'static str,
    port: u16,
}

fn principal(kind: PrincipalKind, id: &str) -> CommandPrincipal {
    CommandPrincipal::new(kind, PrincipalId::new(id).expect("valid principal id"))
}

fn mapper(principal: &CommandPrincipal, command: &Connect) -> Result<Request, MappingError> {
    let id = agent_of(principal)?;
    let own = trogon_policy_eval::Chain::new(vec![trogon_policy_eval::Subject::Agent(id), node("team"), node("org")])
        .expect("non-empty");
    Ok(Request::without_delegation(
        own,
        Action::EgressConnect {
            host: common::host(command.host),
            port: common::port(command.port),
        },
        Phase::ProtectedAction,
    ))
}

fn permitting_snapshot() -> trogon_policy_eval::PolicySnapshot {
    common::builder()
        .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
        .build()
}

type Mapper = fn(&CommandPrincipal, &Connect) -> Result<Request, MappingError>;

fn authorizer_over(
    snapshot: trogon_policy_eval::PolicySnapshot,
    mode: EnforcementMode,
) -> PolicyAuthorizer<Mapper, InMemoryPolicyView, FixedClock> {
    PolicyAuthorizer::new(
        mapper as Mapper,
        view(snapshot),
        Evaluator::new(mode, StalenessBudget::default()),
    )
    .with_clock(FixedClock(common::fresh_now()))
}

fn connect(host: &'static str, port: u16) -> Connect {
    Connect { host, port }
}

fn denial_reason(result: Result<(), AuthorizationDeniedError>) -> String {
    result.expect_err("expected a denial").reason().to_owned()
}

#[test]
fn an_agent_with_a_covering_permit_is_authorized() {
    let authorizer = authorizer_over(permitting_snapshot(), EnforcementMode::Enforce);
    let result = authorizer.authorize(&principal(PrincipalKind::Agent, "a"), &connect("example.com", 443));
    assert_eq!(result, Ok(()));
}

#[test]
fn a_missing_principal_is_the_runtimes_missing_principal_error() {
    let authorizer = authorizer_over(permitting_snapshot(), EnforcementMode::Enforce);
    let result = authorizer.authorize_execution(None, &connect("example.com", 443));
    assert_eq!(result, Err(UnauthorizedError::MissingPrincipal));
}

#[test]
fn a_missing_principal_is_refused_even_when_dry_run_would_admit_everything() {
    let authorizer = authorizer_over(common::empty_snapshot(), EnforcementMode::DryRun);
    let result = authorizer.authorize_execution(None, &connect("example.com", 443));
    assert_eq!(result, Err(UnauthorizedError::MissingPrincipal));
}

#[test]
fn the_runtime_path_wraps_a_denial() {
    let authorizer = authorizer_over(common::empty_snapshot(), EnforcementMode::Enforce);
    let result = authorizer.authorize_execution(
        Some(&principal(PrincipalKind::Agent, "a")),
        &connect("example.com", 443),
    );
    assert!(matches!(result, Err(UnauthorizedError::Denied(_))));
}

#[test]
fn a_denial_names_the_binding_that_caused_it() {
    let missing = authorizer_over(common::empty_snapshot(), EnforcementMode::Enforce);
    let reason = denial_reason(missing.authorize(&principal(PrincipalKind::Agent, "a"), &connect("example.com", 443)));
    assert!(reason.contains("egress permission"), "{reason}");

    let held = authorizer_over(
        common::builder()
            .egress_permission(agent("a"), 1, egress_permission(&[("example.com", 443)]))
            .holds(node("org"), 12, [hold("incident-7", HoldScope::AllProtectedActions)])
            .build(),
        EnforcementMode::Enforce,
    );
    let reason = denial_reason(held.authorize(&principal(PrincipalKind::Agent, "a"), &connect("example.com", 443)));
    assert!(reason.contains("incident-7"), "{reason}");
    assert!(reason.contains("node org"), "{reason}");
    assert!(reason.contains("revision 12"), "{reason}");
}

#[test]
fn principals_that_are_not_agents_have_no_subject_and_are_denied() {
    let authorizer = authorizer_over(permitting_snapshot(), EnforcementMode::Enforce);
    for kind in [PrincipalKind::Person, PrincipalKind::Service] {
        let reason = denial_reason(authorizer.authorize(&principal(kind, "a"), &connect("example.com", 443)));
        assert!(reason.contains("no policy subject"), "{kind}: {reason}");
    }
}

#[test]
fn a_principal_id_that_is_not_an_agent_id_is_denied() {
    let authorizer = authorizer_over(permitting_snapshot(), EnforcementMode::Enforce);
    let reason = denial_reason(authorizer.authorize(
        &principal(PrincipalKind::Agent, "has space"),
        &connect("example.com", 443),
    ));
    assert!(reason.contains("not a valid agent id"), "{reason}");
}

#[test]
fn a_command_the_mapper_does_not_describe_is_denied() {
    fn refusing(_: &CommandPrincipal, _: &Connect) -> Result<Request, MappingError> {
        Err(MappingError::UnsupportedCommand)
    }
    let authorizer = PolicyAuthorizer::new(
        refusing as fn(&CommandPrincipal, &Connect) -> Result<Request, MappingError>,
        view(permitting_snapshot()),
        Evaluator::new(EnforcementMode::Enforce, StalenessBudget::default()),
    )
    .with_clock(FixedClock(common::fresh_now()));
    let reason =
        denial_reason(authorizer.authorize(&principal(PrincipalKind::Agent, "a"), &connect("example.com", 443)));
    assert!(reason.contains("no policy action"), "{reason}");
}

#[test]
fn a_stale_snapshot_is_denied_through_the_authorizer() {
    let authorizer = authorizer_over(permitting_snapshot(), EnforcementMode::Enforce)
        .with_clock(FixedClock(stamped_at() + std::time::Duration::from_secs(400)));
    let reason =
        denial_reason(authorizer.authorize(&principal(PrincipalKind::Agent, "a"), &connect("example.com", 443)));
    assert!(reason.contains("old"), "{reason}");
}

#[derive(Default)]
struct Recording {
    seen: Mutex<Vec<(String, bool, usize)>>,
}

impl DecisionRecorder for Recording {
    fn record(&self, principal: &CommandPrincipal, _request: &Request, decision: &Decision) {
        self.seen.lock().expect("lock").push((
            principal.to_string(),
            decision.would_deny().is_some(),
            decision.consulted().len(),
        ));
    }
}

#[test]
fn dry_run_admits_and_the_recorder_still_sees_the_refusal() {
    let recorder = Arc::new(Recording::default());
    let authorizer = authorizer_over(common::empty_snapshot(), EnforcementMode::DryRun).with_recorder(recorder.clone());

    let result = authorizer.authorize(&principal(PrincipalKind::Agent, "a"), &connect("example.com", 443));
    assert_eq!(result, Ok(()));

    let seen = recorder.seen.lock().expect("lock");
    assert_eq!(seen.as_slice(), &[("agent:a".to_owned(), true, 6)]);
}

#[test]
fn the_recorder_sees_admitted_decisions_too() {
    let recorder = Arc::new(Recording::default());
    let authorizer = authorizer_over(permitting_snapshot(), EnforcementMode::Enforce).with_recorder(recorder.clone());

    assert_eq!(
        authorizer.authorize(&principal(PrincipalKind::Agent, "a"), &connect("example.com", 443)),
        Ok(())
    );
    let seen = recorder.seen.lock().expect("lock");
    assert_eq!(seen.as_slice(), &[("agent:a".to_owned(), false, 6)]);
}

#[test]
fn agent_of_maps_only_agent_principals() {
    assert_eq!(agent_of(&principal(PrincipalKind::Agent, "a")), Ok(agent_id("a")));
    assert_eq!(
        agent_of(&principal(PrincipalKind::Person, "a")),
        Err(MappingError::PrincipalNotAnAgent(PrincipalKind::Person))
    );
    assert_eq!(chain_of_a().subject(), &agent("a"));
}
