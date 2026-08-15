//! Asserts that `decider.replay.events` records the replayed event count.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::convert::Infallible;
use std::time::Duration;

use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter};
use trogon_decider_runtime::{
    CommandExecution, Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity,
    EventType, InMemoryStore, WritePrecondition,
};
use trogon_semconv::metric;

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

    const WRITE_PRECONDITION: Option<WritePrecondition> = Some(WritePrecondition::Any);

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

#[test]
fn replay_events_metric_records_replayed_count() {
    let exporter = InMemoryMetricExporter::default();
    // The interval is long enough that only the explicit `force_flush` below collects. A short
    // interval could export between the two commands, and that collection records the first
    // command's zero replayed events.
    let reader = PeriodicReader::builder(exporter.clone())
        .with_interval(Duration::from_secs(3600))
        .build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();
    opentelemetry::global::set_meter_provider(provider.clone());

    let store = InMemoryStore::new();
    let command = RecordCommand {
        id: "record-1".to_string(),
    };

    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime
        .block_on(CommandExecution::new(&store, &command).execute())
        .expect("first command executes");
    runtime
        .block_on(CommandExecution::new(&store, &command).execute())
        .expect("second command replays the first event");

    provider.force_flush().expect("metrics flush");
    let finished_metrics = exporter.get_finished_metrics().expect("metrics exported");

    // The counter is cumulative, so the last collection carries the total across both commands.
    let data_point = finished_metrics
        .iter()
        .flat_map(|resource_metrics| resource_metrics.scope_metrics())
        .flat_map(|scope_metrics| scope_metrics.metrics())
        .filter(|recorded| recorded.name() == metric::DECIDER_REPLAY_EVENTS)
        .filter_map(|recorded| match recorded.data() {
            AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                sum.data_points().map(|data_point| data_point.value()).next()
            }
            _ => None,
        })
        .last();

    assert_eq!(data_point, Some(1));
}
