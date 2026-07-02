use super::*;
use crate::AgentId;

fn agent(id: &str) -> AgentId {
    AgentId::parse(id).unwrap()
}

#[test]
fn initial_state_is_alive() {
    assert_eq!(initial_state(), KillSwitchState::Alive);
}

#[test]
fn killed_event_transitions_to_killed() {
    let when = OccurredAt::now();
    let next = evolve(
        KillSwitchState::Alive,
        &KillSwitchEvent::AgentKilled {
            agent_id: agent("agent-1"),
            reason: KillReason::BehavioralDrift,
            occurred_at: when,
        },
    );
    assert_eq!(
        next,
        KillSwitchState::Killed {
            reason: KillReason::BehavioralDrift,
            since: when,
        }
    );
}

#[test]
fn revived_event_transitions_to_alive() {
    let killed = KillSwitchState::Killed {
        reason: KillReason::Manual,
        since: OccurredAt::now(),
    };
    let next = evolve(
        killed,
        &KillSwitchEvent::AgentRevived {
            agent_id: agent("agent-1"),
            occurred_at: OccurredAt::now(),
        },
    );
    assert_eq!(next, KillSwitchState::Alive);
}

#[test]
fn a_second_kill_overwrites_the_reason_and_timestamp() {
    let first_kill = OccurredAt::now();
    let state = evolve(
        KillSwitchState::Alive,
        &KillSwitchEvent::AgentKilled {
            agent_id: agent("agent-1"),
            reason: KillReason::RateLimit,
            occurred_at: first_kill,
        },
    );
    let second_kill = OccurredAt::now();
    let state = evolve(
        state,
        &KillSwitchEvent::AgentKilled {
            agent_id: agent("agent-1"),
            reason: KillReason::RingBreach,
            occurred_at: second_kill,
        },
    );
    assert_eq!(
        state,
        KillSwitchState::Killed {
            reason: KillReason::RingBreach,
            since: second_kill,
        }
    );
}
