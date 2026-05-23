use a2a_types::TaskPushNotificationConfig;

use crate::push::delivery_semantics::DeliverySemantics;

#[derive(Clone, Debug, Eq, PartialEq)]
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
