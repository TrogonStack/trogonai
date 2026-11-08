use super::*;

#[test]
fn from_dotted_suffix_resolves_each_variant() {
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
        assert_eq!(expected.as_dotted_suffix(), suffix);
    }
}

#[test]
fn all_round_trips_every_variant_through_its_dotted_suffix() {
    for method in A2aMethod::ALL {
        assert_eq!(A2aMethod::from_dotted_suffix(method.as_dotted_suffix()), Some(method));
    }
}

#[test]
fn from_dotted_suffix_unknown_returns_none() {
    assert_eq!(A2aMethod::from_dotted_suffix("unknown.method"), None);
    assert_eq!(A2aMethod::from_dotted_suffix(""), None);
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

#[test]
fn all_round_trips_every_variant_through_its_jsonrpc_name() {
    for method in A2aMethod::ALL {
        assert_eq!(A2aMethod::from_jsonrpc_name(method.as_str()), Some(method));
    }
}

#[test]
fn from_jsonrpc_name_rejects_names_outside_the_method_set() {
    for name in ["", "message.send", "tasks/subscribe", "Message/Send"] {
        assert_eq!(A2aMethod::from_jsonrpc_name(name), None);
    }
}
