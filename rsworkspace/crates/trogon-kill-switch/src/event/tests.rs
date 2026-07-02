use super::*;

fn agent(id: &str) -> AgentId {
    AgentId::parse(id).unwrap()
}

#[test]
fn agent_id_reads_killed_event() {
    let event = KillSwitchEvent::AgentKilled {
        agent_id: agent("agent-1"),
        reason: KillReason::Manual,
        occurred_at: OccurredAt::now(),
    };
    assert_eq!(event.agent_id(), &agent("agent-1"));
}

#[test]
fn agent_id_reads_revived_event() {
    let event = KillSwitchEvent::AgentRevived {
        agent_id: agent("agent-2"),
        occurred_at: OccurredAt::now(),
    };
    assert_eq!(event.agent_id(), &agent("agent-2"));
}

#[test]
fn occurred_at_reads_from_either_variant() {
    let when = OccurredAt::now();
    let killed = KillSwitchEvent::AgentKilled {
        agent_id: agent("agent-1"),
        reason: KillReason::RingBreach,
        occurred_at: when,
    };
    let revived = KillSwitchEvent::AgentRevived {
        agent_id: agent("agent-1"),
        occurred_at: when,
    };
    assert_eq!(killed.occurred_at(), when);
    assert_eq!(revived.occurred_at(), when);
}
