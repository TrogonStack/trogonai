//! Composite `PushDispatcher` that routes to the http / nats / jetstream impl
//! by the parsed [`PushNotificationTarget`].

use std::sync::Arc;

use a2a::types::TaskPushNotificationConfig;

use crate::push::delivery_semantics::DeliverySemantics;
use crate::push::dispatch_error::DispatchError;
use crate::push::dispatcher::{
    HttpPushDispatcher, JetStreamPublishPushDispatcher, NatsPublishPushDispatcher, PushDispatcher,
};
use crate::push::push_notification_target::PushNotificationTarget;
use crate::push::terminal_push_task_state::TerminalPushTaskState;
use crate::task_id::A2aTaskId;

pub struct CompositePushDispatcher {
    http: HttpPushDispatcher,
    nats: Box<dyn PushDispatcher>,
    jetstream: Box<dyn PushDispatcher>,
}

impl CompositePushDispatcher {
    pub fn new(http: HttpPushDispatcher, nats: Box<dyn PushDispatcher>, jetstream: Box<dyn PushDispatcher>) -> Self {
        Self { http, nats, jetstream }
    }
}

#[async_trait::async_trait]
impl PushDispatcher for CompositePushDispatcher {
    async fn dispatch(
        &self,
        task_id: &A2aTaskId,
        config: &TaskPushNotificationConfig,
        delivery_semantics: DeliverySemantics,
        terminal_task_state: TerminalPushTaskState,
        payload: &[u8],
    ) -> Result<(), DispatchError> {
        match PushNotificationTarget::parse(&config.url)? {
            PushNotificationTarget::Http(_) => {
                self.http
                    .dispatch(task_id, config, delivery_semantics, terminal_task_state, payload)
                    .await
            }
            PushNotificationTarget::Nats(_) => {
                self.nats
                    .dispatch(task_id, config, delivery_semantics, terminal_task_state, payload)
                    .await
            }
            PushNotificationTarget::JetStream(_) => {
                self.jetstream
                    .dispatch(task_id, config, delivery_semantics, terminal_task_state, payload)
                    .await
            }
        }
    }
}

pub fn composite_push_dispatcher<N, J>(nats: N, jetstream: J, http_client: reqwest::Client) -> Arc<dyn PushDispatcher>
where
    N: trogon_nats::PublishClient + Clone + Send + Sync + 'static,
    J: trogon_nats::jetstream::JetStreamPublisher + Clone + Send + Sync + 'static,
{
    Arc::new(CompositePushDispatcher::new(
        HttpPushDispatcher::new(http_client),
        Box::new(NatsPublishPushDispatcher::new(nats)),
        Box::new(JetStreamPublishPushDispatcher::new(jetstream)),
    ))
}

#[cfg(test)]
mod tests;
