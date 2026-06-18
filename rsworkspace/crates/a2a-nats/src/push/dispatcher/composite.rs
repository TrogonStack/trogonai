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
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test dispatcher that records which leg was invoked, so the composite's
    /// routing rules can be asserted without spinning up real transports.
    #[derive(Default)]
    struct RecordingLeg {
        tag: &'static str,
        calls: Mutex<Vec<&'static str>>,
    }

    #[async_trait::async_trait]
    impl PushDispatcher for RecordingLeg {
        async fn dispatch(
            &self,
            _task_id: &A2aTaskId,
            _config: &TaskPushNotificationConfig,
            _delivery_semantics: DeliverySemantics,
            _terminal_task_state: TerminalPushTaskState,
            _payload: &[u8],
        ) -> Result<(), DispatchError> {
            self.calls.lock().unwrap().push(self.tag);
            Ok(())
        }
    }

    fn config(url: &str) -> TaskPushNotificationConfig {
        TaskPushNotificationConfig {
            url: url.into(),
            id: Some("cfg-1".into()),
            task_id: String::new(),
            token: None,
            authentication: None,
            tenant: None,
        }
    }

    fn task() -> A2aTaskId {
        A2aTaskId::new("task-1").unwrap()
    }

    fn http_leg() -> HttpPushDispatcher {
        HttpPushDispatcher::new(reqwest::Client::new())
    }

    #[tokio::test]
    async fn http_target_routes_to_http_leg() {
        // The HTTP leg is a real HttpPushDispatcher pointed at a reserved
        // port; the dispatcher returns Err on transport refusal, but the
        // routing decision has already happened by then — we assert that
        // the nats/jetstream legs were NOT touched.
        let nats = Arc::new(RecordingLeg {
            tag: "nats",
            ..Default::default()
        });
        let js = Arc::new(RecordingLeg {
            tag: "js",
            ..Default::default()
        });
        let composite = CompositePushDispatcher::new(
            http_leg(),
            Box::new(NatsBox(nats.clone())),
            Box::new(NatsBox(js.clone())),
        );

        let _ = composite
            .dispatch(
                &task(),
                &config("http://127.0.0.1:1/hook"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await;

        assert!(nats.calls.lock().unwrap().is_empty());
        assert!(js.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn nats_target_routes_to_nats_leg() {
        let nats = Arc::new(RecordingLeg {
            tag: "nats",
            ..Default::default()
        });
        let js = Arc::new(RecordingLeg {
            tag: "js",
            ..Default::default()
        });
        let composite = CompositePushDispatcher::new(
            http_leg(),
            Box::new(NatsBox(nats.clone())),
            Box::new(NatsBox(js.clone())),
        );

        composite
            .dispatch(
                &task(),
                &config("subject:a2a.push.bot.caller.t1"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap();

        assert_eq!(*nats.calls.lock().unwrap(), vec!["nats"]);
        assert!(js.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn jetstream_target_routes_to_jetstream_leg() {
        let nats = Arc::new(RecordingLeg {
            tag: "nats",
            ..Default::default()
        });
        let js = Arc::new(RecordingLeg {
            tag: "js",
            ..Default::default()
        });
        let composite = CompositePushDispatcher::new(
            http_leg(),
            Box::new(NatsBox(nats.clone())),
            Box::new(NatsBox(js.clone())),
        );

        composite
            .dispatch(
                &task(),
                &config("jetstream:a2a.push.bot.caller.t1"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap();

        assert_eq!(*js.calls.lock().unwrap(), vec!["js"]);
        assert!(nats.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unparseable_url_surfaces_invalid_target_error() {
        let nats = Arc::new(RecordingLeg {
            tag: "nats",
            ..Default::default()
        });
        let js = Arc::new(RecordingLeg {
            tag: "js",
            ..Default::default()
        });
        let composite = CompositePushDispatcher::new(
            http_leg(),
            Box::new(NatsBox(nats.clone())),
            Box::new(NatsBox(js.clone())),
        );

        let err = composite
            .dispatch(
                &task(),
                &config("ftp://example.com/hook"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
        assert!(nats.calls.lock().unwrap().is_empty());
        assert!(js.calls.lock().unwrap().is_empty());
    }

    /// Thin newtype that wraps an `Arc<RecordingLeg>` so it satisfies the
    /// `Box<dyn PushDispatcher>` slot without giving up the shared handle
    /// the test uses to assert on call counts.
    struct NatsBox(Arc<RecordingLeg>);

    #[async_trait::async_trait]
    impl PushDispatcher for NatsBox {
        async fn dispatch(
            &self,
            task_id: &A2aTaskId,
            config: &TaskPushNotificationConfig,
            delivery_semantics: DeliverySemantics,
            terminal_task_state: TerminalPushTaskState,
            payload: &[u8],
        ) -> Result<(), DispatchError> {
            self.0
                .dispatch(task_id, config, delivery_semantics, terminal_task_state, payload)
                .await
        }
    }

    #[tokio::test]
    async fn factory_returns_dyn_dispatcher_routing_by_target() {
        use trogon_nats::AdvancedMockNatsClient;

        let nats = AdvancedMockNatsClient::new();
        let js = trogon_nats::jetstream::mocks::MockJetStreamPublisher::new();
        let dispatcher = composite_push_dispatcher(nats.clone(), js, reqwest::Client::new());

        dispatcher
            .dispatch(
                &task(),
                &config("subject:a2a.push.bot.caller.t1"),
                DeliverySemantics::AtLeastOnce,
                TerminalPushTaskState::Completed,
                b"{}",
            )
            .await
            .unwrap();
        assert_eq!(nats.published_messages(), vec!["a2a.push.bot.caller.t1"]);
    }
}
