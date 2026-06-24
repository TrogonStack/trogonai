use super::*;

#[test]
fn new_pairs_task_push_with_semantics() {
    let task_push = TaskPushNotificationConfig {
        url: "https://example.com/hook".into(),
        id: Some("cfg-1".into()),
        task_id: "task-1".into(),
        token: None,
        authentication: None,
        tenant: None,
    };
    let semantics = DeliverySemantics::default();
    let config = PushNotificationConfig::new(task_push.clone(), semantics.clone());
    assert_eq!(config.task_push, task_push);
    assert_eq!(config.delivery_semantics, semantics);
}
