use super::*;

#[test]
fn parses_every_known_wire_method() {
    assert_eq!(parse_request_method("message/send"), Ok(A2aMethod::MessageSend));
    assert_eq!(parse_request_method("message/stream"), Ok(A2aMethod::MessageStream));
    assert_eq!(parse_request_method("tasks/get"), Ok(A2aMethod::TasksGet));
    assert_eq!(parse_request_method("tasks/list"), Ok(A2aMethod::TasksList));
    assert_eq!(parse_request_method("tasks/cancel"), Ok(A2aMethod::TasksCancel));
    assert_eq!(
        parse_request_method("tasks/resubscribe"),
        Ok(A2aMethod::TasksResubscribe)
    );
    assert_eq!(
        parse_request_method("tasks/pushNotificationConfig/set"),
        Ok(A2aMethod::PushNotificationSet)
    );
    assert_eq!(
        parse_request_method("tasks/pushNotificationConfig/get"),
        Ok(A2aMethod::PushNotificationGet)
    );
    assert_eq!(
        parse_request_method("tasks/pushNotificationConfig/list"),
        Ok(A2aMethod::PushNotificationList)
    );
    assert_eq!(
        parse_request_method("tasks/pushNotificationConfig/delete"),
        Ok(A2aMethod::PushNotificationDelete)
    );
    assert_eq!(parse_request_method("agent/card"), Ok(A2aMethod::AgentCard));
}

#[test]
fn rejects_unknown_method() {
    let err = parse_request_method("message/unknown").expect_err("unknown method rejected");
    assert_eq!(err, RequestMethodError("message/unknown".to_string()));
    assert_eq!(err.to_string(), "unknown a2a request method 'message/unknown'");
}
