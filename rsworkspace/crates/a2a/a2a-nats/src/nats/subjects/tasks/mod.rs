//! Task-scoped subjects (`{prefix}.tasks.{task_id}.events.{seq}`).

pub mod events;

pub use events::TaskEventsSubject;
