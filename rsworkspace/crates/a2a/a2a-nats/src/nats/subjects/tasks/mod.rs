//! Task-scoped subjects (`{prefix}.v1.tasks.{task_id}.{kind}.{id}.events`), where
//! `{kind}.{id}` is the principal of the caller the task runs for (ADR#0082).

pub mod events;

pub use events::{TaskEventsSubject, TaskEventsSubjectError};
