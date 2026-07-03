//! Bridge-side push notification config envelope.
//!
//! `task_push` is intentionally the raw `a2a::types::TaskPushNotificationConfig`
//! wire shape so this slice can land without first introducing a parallel
//! domain type for every field of the upstream spec. The validated
//! domain projection (parsed URL via `WebhookUrl` / `NatsPushSubject`,
//! validated `PushNotificationConfigId`, normalised authentication scheme)
//! lands with the push dispatcher PR alongside the conversion-on-ingress
//! sites that will consume it.

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
mod tests;
