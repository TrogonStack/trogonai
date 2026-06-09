//! Schedule-event processors.
//!
//! `processor` is the namespace for the schedulers's event processors. Today it
//! holds one — [`execution`], which reconciles persisted schedule events into
//! NATS execution schedules. Future processors (for example an RRULE recurrence
//! controller) live as sibling modules here.

pub mod execution;
