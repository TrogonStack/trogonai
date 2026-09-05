use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct ProjectionFailureObserver {
    position: Arc<Mutex<Option<u64>>>,
}

impl ProjectionFailureObserver {
    pub fn position(&self) -> Option<u64> {
        *self.position.lock().unwrap()
    }
}

struct FailureFields<'a>(&'a mut Option<u64>);

impl tracing::field::Visit for FailureFields<'_> {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "stream_position" {
            *self.0 = Some(value);
        }
    }

    fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {}
}

impl tracing::Subscriber for ProjectionFailureObserver {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::ERROR && metadata.fields().field("stream_position").is_some()
    }

    fn new_span(&self, _attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        event.record(&mut FailureFields(&mut self.position.lock().unwrap()));
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}
