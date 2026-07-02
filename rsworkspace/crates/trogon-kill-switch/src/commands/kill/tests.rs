use trogon_decider::testing::TestCase;

use super::*;

fn agent(id: &str) -> AgentId {
    AgentId::parse(id).unwrap()
}

fn kill_command(id: &str, reason: KillReason, occurred_at: OccurredAt) -> Kill {
    Kill::new(agent(id), reason, occurred_at)
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
fn given_when_then_kills_a_live_agent() {
    let when = OccurredAt::now();
    TestCase::<Kill>::new()
        .given_no_history()
        .when(kill_command("agent-1", KillReason::Manual, when))
        .then([killed("agent-1", KillReason::Manual, when)]);
}

#[test]
fn given_when_then_kills_a_revived_agent_again() {
    let first_kill = OccurredAt::now();
    let revive = OccurredAt::now();
    let second_kill = OccurredAt::now();
    TestCase::<Kill>::new()
        .given([killed("agent-1", KillReason::RateLimit, first_kill)])
        .given([revived("agent-1", revive)])
        .when(kill_command("agent-1", KillReason::RingBreach, second_kill))
        .then([killed("agent-1", KillReason::RingBreach, second_kill)]);
}

#[test]
fn given_when_then_rejects_killing_an_already_killed_agent() {
    let first_kill = OccurredAt::now();
    TestCase::<Kill>::new()
        .given([killed("agent-1", KillReason::BehavioralDrift, first_kill)])
        .when(kill_command("agent-1", KillReason::Manual, OccurredAt::now()))
        .then_error(KillError::AlreadyKilled {
            existing_reason: KillReason::BehavioralDrift,
        });
}

#[test]
fn kill_on_killed_is_idempotent_regardless_of_reason() {
    for existing in [
        KillReason::BehavioralDrift,
        KillReason::RateLimit,
        KillReason::RingBreach,
        KillReason::Manual,
    ] {
        let first_kill = OccurredAt::now();
        TestCase::<Kill>::new()
            .given([killed("agent-1", existing, first_kill)])
            .when(kill_command("agent-1", KillReason::Manual, OccurredAt::now()))
            .then_error(KillError::AlreadyKilled {
                existing_reason: existing,
            });
    }
}

#[test]
fn decide_error_code_is_stable_for_already_killed() {
    let error = KillError::AlreadyKilled {
        existing_reason: KillReason::Manual,
    };
    assert_eq!(Kill::decide_error_code(&error), "already-killed");
}

#[test]
fn error_message_names_the_existing_reason() {
    let error = KillError::AlreadyKilled {
        existing_reason: KillReason::RingBreach,
    };
    assert_eq!(
        error.to_string(),
        "agent is already killed for reason 'ring_breach'; kill is idempotent, no event emitted"
    );
}

#[test]
fn stream_id_is_the_agent_id() {
    let command = kill_command("agent-42", KillReason::Manual, OccurredAt::now());
    assert_eq!(command.stream_id(), "agent-42");
}
