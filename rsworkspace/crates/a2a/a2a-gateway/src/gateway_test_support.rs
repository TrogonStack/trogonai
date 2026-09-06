use std::sync::Arc;

use a2a_nats::nats::subjects::tasks::TaskEventsSubject;
use a2a_nats::{A2aPrefix, A2aTaskId};
use async_nats::jetstream::AckKind;
use async_nats::subject::ToSubject;
use trogon_nats::jetstream::message::{JsAck, JsAckWith, JsDoubleAck, JsMessageRef};
use trogon_nats::jetstream::mocks::{AckKindSnapshot, MockJsMessage};
use trogon_nats::mocks::MockError;
use trogon_std::log_capture::{CapturedEvents, CapturedLogs, LevelFilter};

pub(crate) enum Diagnostics {
    Facade(CapturedLogs),
    Tracing {
        events: CapturedEvents,
        _guard: tracing::subscriber::DefaultGuard,
    },
}

impl Diagnostics {
    pub(crate) fn both_outputs() -> Self {
        match CapturedLogs::isolated() {
            Some(logs) => Self::Facade(logs),
            None => {
                let events = CapturedEvents::new();
                let guard = events.install(LevelFilter::DEBUG);
                Self::Tracing { events, _guard: guard }
            }
        }
    }

    pub(crate) fn assert_event(&self, message: &str, fields: &[(&str, &str)]) {
        match self {
            Self::Facade(logs) => {
                let messages: Vec<_> = logs.records().into_iter().map(|record| record.message).collect();
                assert!(
                    messages.iter().any(|record| record.contains(message)
                        && fields.iter().all(|(name, value)| {
                            record.contains(&format!("{name}={value}")) || record.contains(&format!("{name}={value:?}"))
                        })),
                    "missing diagnostic {message:?} with {fields:?}: {messages:?}"
                );
            }
            Self::Tracing { events, .. } => {
                let events = events.events();
                assert!(
                    events.iter().any(|event| event.message() == Some(message)
                        && fields.iter().all(|(name, value)| event.field(name) == Some(*value))),
                    "missing diagnostic {message:?} with {fields:?}: {events:?}"
                );
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct ObservedMessage(Arc<MockJsMessage>);

impl ObservedMessage {
    pub(crate) fn new(message: async_nats::Message) -> Self {
        Self(Arc::new(MockJsMessage::new(message)))
    }

    pub(crate) fn with_failing_signals(message: async_nats::Message) -> Self {
        Self(Arc::new(MockJsMessage::with_failing_signals(message)))
    }

    pub(crate) fn signals(&self) -> Vec<AckKindSnapshot> {
        self.0.signals()
    }
}

impl JsMessageRef for ObservedMessage {
    fn message(&self) -> &async_nats::Message {
        self.0.message()
    }
}

impl JsAck for ObservedMessage {
    type Error = MockError;

    async fn ack(&self) -> Result<(), Self::Error> {
        self.0.ack().await
    }
}

impl JsAckWith for ObservedMessage {
    type Error = MockError;

    async fn ack_with(&self, kind: AckKind) -> Result<(), Self::Error> {
        self.0.ack_with(kind).await
    }
}

impl JsDoubleAck for ObservedMessage {
    type Error = MockError;

    async fn double_ack(&self) -> Result<(), Self::Error> {
        self.0.double_ack().await
    }
}

pub(crate) fn event(headers: async_nats::HeaderMap) -> async_nats::Message {
    let payload = bytes::Bytes::from_static(b"event payload");
    let prefix = A2aPrefix::new("a2a").expect("test prefix");
    let task_id = A2aTaskId::new("task-1").expect("test task");
    async_nats::Message {
        subject: TaskEventsSubject::new(&prefix, &task_id).to_subject(),
        reply: None,
        payload: payload.clone(),
        headers: Some(headers),
        status: None,
        description: None,
        length: payload.len(),
    }
}
