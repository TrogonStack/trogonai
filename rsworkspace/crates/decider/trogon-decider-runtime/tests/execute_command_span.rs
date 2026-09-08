//! Asserts that `decider.execute_command` carries its stream, precondition, and outcome attributes.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::borrow::Cow;
use std::convert::Infallible;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tracing_subscriber::layer::SubscriberExt;
use trogon_decider_runtime::{
    CommandExecution, Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity,
    EventType, InMemoryStore, WritePrecondition,
};
use trogon_semconv::{attribute, span};

#[derive(Debug, Clone)]
struct RecordCommand {
    id: String,
}

#[derive(Debug, Clone)]
struct RecordEvent;

impl Decider for RecordCommand {
    type StreamId = str;
    type State = ();
    type Event = RecordEvent;
    type DecideError = Infallible;
    type EvolveError = Infallible;

    #[cfg_attr(
        dylint_lib = "trogon_lints",
        allow(
            weakened_write_precondition,
            reason = "the fixture exists to observe telemetry, and appends against a stream no other writer touches"
        )
    )]
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::Any;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }

    fn initial_state() -> Self::State {}

    fn evolve(_state: Self::State, _event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(())
    }

    fn decide(_state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::event(RecordEvent))
    }
}

impl EventIdentity for RecordEvent {}

impl EventType for RecordEvent {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("recorded")
    }
}

impl EventEncode for RecordEvent {
    type Error = Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(Vec::new())
    }
}

impl EventDecode for RecordEvent {
    type Error = Infallible;

    fn decode(_event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        Ok(EventDecodeOutcome::Decoded(RecordEvent))
    }
}

fn attribute_value<'a>(attributes: &'a [KeyValue], key: &str) -> Option<Cow<'a, str>> {
    attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.as_str())
}

#[test]
fn execute_command_span_records_stream_and_decision_attributes() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("trogon-decider-runtime-test");
    let subscriber = tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));

    let store = InMemoryStore::new();
    let command = RecordCommand {
        id: "record-1".to_string(),
    };

    tracing::subscriber::with_default(subscriber, || {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime
            .block_on(CommandExecution::new(&store, &command).execute())
            .expect("command executes");
    });

    // `InMemorySpanExporter::shutdown` resets its stored spans by default, so
    // the finished spans must be read before shutting down the provider.
    let spans = exporter.get_finished_spans().expect("spans exported");
    provider.shutdown().expect("provider shuts down");

    let recorded = spans
        .iter()
        .find(|recorded| recorded.name == span::DECIDER_EXECUTE_COMMAND)
        .expect("decider.execute_command span recorded");

    assert_eq!(
        attribute_value(&recorded.attributes, attribute::STREAM_ID).as_deref(),
        Some("record-1"),
    );
    assert_eq!(
        attribute_value(&recorded.attributes, attribute::WRITE_PRECONDITION).as_deref(),
        Some("any"),
    );
    assert_eq!(
        attribute_value(&recorded.attributes, attribute::DECISION_OUTCOME).as_deref(),
        Some("decided"),
    );
}
