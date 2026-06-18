use a2a::types::TaskPushNotificationConfig;

use crate::push::delivery_semantics::DeliverySemantics;

#[derive(Clone, Debug, PartialEq)]
pub struct PushNotificationConfig {
    pub delivery_semantics: DeliverySemantics,
    pub task_push: TaskPushNotificationConfig,
}

impl PushNotificationConfig {
    pub fn new(task_push: TaskPushNotificationConfig, delivery_semantics: DeliverySemantics) -> Self {
        Self {
            task_push,
            delivery_semantics,
        }
    }
}

#[cfg(test)]
mod tests {
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
}
