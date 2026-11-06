//! Field-aware capture of `tracing` events for tests.
//!
//! A test that reads a subscriber's rendered output cannot tell a value that
//! was recorded as a field from one that was only interpolated into the
//! message, because both end up in the same text. [`CapturedEvents`] keeps the
//! fields apart from the message so an assertion can name exactly which one it
//! expects.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::subscriber::{DefaultGuard, NoSubscriber};
use tracing::{Dispatch, Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

pub use tracing_subscriber::filter::LevelFilter;

mod facade;
pub use facade::{CapturedLog, CapturedLogs, LogLevel};

/// A single `tracing` event, recorded as its fields rather than as rendered
/// text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedEvent {
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// The field `tracing` synthesizes for an event's message.
    const MESSAGE_FIELD: &'static str = "message";

    /// The value recorded for the field `name`, or `None` when the event
    /// carried no such field. A value that only reached the message returns
    /// `None`, which is what makes this worth asserting on.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// The event's message.
    pub fn message(&self) -> Option<&str> {
        self.field(Self::MESSAGE_FIELD)
    }
}

/// Collects the events emitted while it is installed as the default
/// subscriber.
#[derive(Clone, Default)]
pub struct CapturedEvents(Arc<Mutex<Vec<CapturedEvent>>>);

impl CapturedEvents {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `self` the default subscriber for the current thread until the
    /// returned guard is dropped, recording events at or below `max_level`.
    ///
    /// `tracing` builds an event's field values only when a subscriber is
    /// listening at that level, so the level has to reach the callsite under
    /// test or nothing is captured.
    pub fn install(&self, max_level: LevelFilter) -> DefaultGuard {
        ask_every_dispatcher_for_interest();
        tracing_subscriber::registry()
            .with(max_level)
            .with(self.clone())
            .set_default()
    }

    /// The events recorded so far, in the order they were emitted.
    pub fn events(&self) -> Vec<CapturedEvent> {
        self.0.lock().expect("captured events are not poisoned").clone()
    }
}

/// While a single dispatcher is registered, `tracing` caches a callsite's
/// interest from whatever is the default on the thread that reaches it first.
/// A callsite first reached on a test thread with no subscriber is then cached
/// as never, and a capture installed on another thread misses its events. A
/// second dispatcher that lives for the whole process makes `tracing` ask every
/// registered dispatcher instead.
fn ask_every_dispatcher_for_interest() {
    static NEVER_INTERESTED: OnceLock<Dispatch> = OnceLock::new();
    NEVER_INTERESTED.get_or_init(|| Dispatch::new(NoSubscriber::default()));
}

impl<S: Subscriber> Layer<S> for CapturedEvents {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = FieldCollector(BTreeMap::new());
        event.record(&mut fields);
        self.0
            .lock()
            .expect("captured events are not poisoned")
            .push(CapturedEvent { fields: fields.0 });
    }
}

struct FieldCollector(BTreeMap<String, String>);

impl Visit for FieldCollector {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    /// Covers the `%` and `?` sigils as well as the message, all of which
    /// `tracing` records through `Debug`.
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

#[cfg(test)]
mod tests;
