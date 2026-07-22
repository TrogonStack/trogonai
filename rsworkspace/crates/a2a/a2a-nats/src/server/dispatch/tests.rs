use super::*;

fn prefix_len(prefix: &str, agent_id: &str) -> usize {
    format!("{prefix}.agents.{agent_id}").len()
}

#[test]
fn message_send_resolves() {
    let pl = prefix_len("a2a", "planner");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.planner.message.send", pl),
        Some(A2aMethod::MessageSend)
    );
}

#[test]
fn message_stream_resolves() {
    let pl = prefix_len("a2a", "planner");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.planner.message.stream", pl),
        Some(A2aMethod::MessageStream)
    );
}

#[test]
fn tasks_get_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.tasks.get", pl),
        Some(A2aMethod::TasksGet)
    );
}

#[test]
fn tasks_list_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.tasks.list", pl),
        Some(A2aMethod::TasksList)
    );
}

#[test]
fn tasks_cancel_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.tasks.cancel", pl),
        Some(A2aMethod::TasksCancel)
    );
}

#[test]
fn tasks_resubscribe_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.tasks.resubscribe", pl),
        Some(A2aMethod::TasksResubscribe)
    );
}

#[test]
fn push_set_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.push.set", pl),
        Some(A2aMethod::PushNotificationSet)
    );
}

#[test]
fn push_get_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.push.get", pl),
        Some(A2aMethod::PushNotificationGet)
    );
}

#[test]
fn push_list_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.push.list", pl),
        Some(A2aMethod::PushNotificationList)
    );
}

#[test]
fn push_delete_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.push.delete", pl),
        Some(A2aMethod::PushNotificationDelete)
    );
}

#[test]
fn agent_card_resolves() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(
        A2aMethod::from_subject("a2a.agents.bot.card", pl),
        Some(A2aMethod::AgentCard)
    );
}

#[test]
fn unknown_suffix_returns_none() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(A2aMethod::from_subject("a2a.agents.bot.unknown.method", pl), None);
}

#[test]
fn too_short_subject_returns_none() {
    let pl = prefix_len("a2a", "bot");
    assert_eq!(A2aMethod::from_subject("a2a.agents.bot", pl), None);
}

#[test]
fn from_dotted_suffix_resolves_each_variant() {
    // Callers that have already extracted `method_dots` from a
    // subject (e.g. via `gateway_ingress_agent_and_method_dots`)
    // shouldn't have to rebuild a full subject + recompute
    // `prefix_len` to reach the typed enum. Cover every suffix the
    // gateway dispatches on.
    let pairs = [
        ("message.send", A2aMethod::MessageSend),
        ("message.stream", A2aMethod::MessageStream),
        ("tasks.get", A2aMethod::TasksGet),
        ("tasks.list", A2aMethod::TasksList),
        ("tasks.cancel", A2aMethod::TasksCancel),
        ("tasks.resubscribe", A2aMethod::TasksResubscribe),
        ("push.set", A2aMethod::PushNotificationSet),
        ("push.get", A2aMethod::PushNotificationGet),
        ("push.list", A2aMethod::PushNotificationList),
        ("push.delete", A2aMethod::PushNotificationDelete),
        ("card", A2aMethod::AgentCard),
    ];
    for (suffix, expected) in pairs {
        assert_eq!(A2aMethod::from_dotted_suffix(suffix), Some(expected.clone()));
    }
}

#[test]
fn from_dotted_suffix_unknown_returns_none() {
    assert_eq!(A2aMethod::from_dotted_suffix("unknown.method"), None);
    assert_eq!(A2aMethod::from_dotted_suffix(""), None);
}

#[test]
fn from_subject_and_from_dotted_suffix_agree() {
    // Round-trip sanity: when both entry points see the same suffix,
    // they must resolve to the same variant. Guards against drift
    // between the two match tables.
    let pl = prefix_len("a2a", "bot");
    let cases = [
        ("a2a.agents.bot.message.send", "message.send"),
        ("a2a.agents.bot.tasks.get", "tasks.get"),
        ("a2a.agents.bot.push.list", "push.list"),
        ("a2a.agents.bot.card", "card"),
    ];
    for (subject, suffix) in cases {
        assert_eq!(
            A2aMethod::from_subject(subject, pl),
            A2aMethod::from_dotted_suffix(suffix)
        );
    }
}

#[test]
fn as_str_covers_every_variant() {
    let pairs = [
        (A2aMethod::MessageSend, "message/send"),
        (A2aMethod::MessageStream, "message/stream"),
        (A2aMethod::TasksGet, "tasks/get"),
        (A2aMethod::TasksList, "tasks/list"),
        (A2aMethod::TasksCancel, "tasks/cancel"),
        (A2aMethod::TasksResubscribe, "tasks/resubscribe"),
        (A2aMethod::PushNotificationSet, "tasks/pushNotificationConfig/set"),
        (A2aMethod::PushNotificationGet, "tasks/pushNotificationConfig/get"),
        (A2aMethod::PushNotificationList, "tasks/pushNotificationConfig/list"),
        (A2aMethod::PushNotificationDelete, "tasks/pushNotificationConfig/delete"),
        (A2aMethod::AgentCard, "agent/card"),
    ];
    for (method, expected) in pairs {
        assert_eq!(method.as_str(), expected);
    }
}
