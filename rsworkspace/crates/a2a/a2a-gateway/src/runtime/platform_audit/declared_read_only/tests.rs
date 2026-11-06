use super::*;

#[test]
fn reads_are_declared_read_only() {
    for method in [
        A2aMethod::TasksGet,
        A2aMethod::TasksList,
        A2aMethod::PushNotificationGet,
        A2aMethod::PushNotificationList,
        A2aMethod::AgentCard,
    ] {
        assert_eq!(declared_read_only(&method), ReadOnly::Declared, "{method:?}");
    }
}

#[test]
fn everything_that_acts_on_the_agent_is_undeclared() {
    for method in [
        A2aMethod::MessageSend,
        A2aMethod::MessageStream,
        A2aMethod::TasksCancel,
        A2aMethod::TasksResubscribe,
        A2aMethod::PushNotificationSet,
        A2aMethod::PushNotificationDelete,
    ] {
        assert_eq!(declared_read_only(&method), ReadOnly::Undeclared, "{method:?}");
    }
}
