use trogon_decider::testing::TestCase;

use super::*;
use crate::KillReason;

fn agent(id: &str) -> AgentId {
    AgentId::parse(id).unwrap()
}

fn revive_command(id: &str, occurred_at: OccurredAt) -> Revive {
    Revive::new(agent(id), occurred_at)
}

fn killed(id: &str, reason: KillReason, occurred_at: OccurredAt) -> KillSwitchEvent {
    KillSwitchEvent::AgentKilled {
        agent_id: agent(id),
        reason,
        occurred_at,
    }
}

fn revived(id: &str, occurred_at: OccurredAt) -> KillSwitchEvent {
    KillSwitchEvent::AgentRevived {
        agent_id: agent(id),
        occurred_at,
    }
}

#[test]
fn given_when_then_revives_a_killed_agent() {
    let kill_at = OccurredAt::now();
    let revive_at = OccurredAt::now();
    TestCase::<Revive>::new()
        .given([killed("agent-1", KillReason::Manual, kill_at)])
        .when(revive_command("agent-1", revive_at))
        .then([revived("agent-1", revive_at)]);
}

#[test]
fn given_when_then_rejects_reviving_a_live_agent() {
    TestCase::<Revive>::new()
        .given_no_history()
        .when(revive_command("agent-1", OccurredAt::now()))
        .then_error(ReviveError::AlreadyAlive);
}

#[test]
fn revive_on_alive_is_idempotent_after_a_prior_revive() {
    let kill_at = OccurredAt::now();
    let first_revive = OccurredAt::now();
    TestCase::<Revive>::new()
        .given([killed("agent-1", KillReason::RateLimit, kill_at)])
        .given([revived("agent-1", first_revive)])
        .when(revive_command("agent-1", OccurredAt::now()))
        .then_error(ReviveError::AlreadyAlive);
}

#[test]
fn decide_error_code_is_stable_for_already_alive() {
    assert_eq!(Revive::decide_error_code(&ReviveError::AlreadyAlive), "already-alive");
}

#[test]
fn error_message_is_human_readable() {
    assert_eq!(
        ReviveError::AlreadyAlive.to_string(),
        "agent is already alive; revive is idempotent, no event emitted"
    );
}

#[test]
fn stream_id_is_the_agent_id() {
    let command = revive_command("agent-42", OccurredAt::now());
    assert_eq!(command.stream_id(), "agent-42");
}
