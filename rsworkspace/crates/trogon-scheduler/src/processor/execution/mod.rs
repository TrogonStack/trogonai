//! The execution-schedule processor.
//!
//! This processor consumes persisted `v1::ScheduleEvent` records and reconciles
//! enabled `At`/`Every`/`Cron` definitions into NATS execution schedules. It
//! owns everything for that one concern: recorded event decoding, pure
//! reconciliation rules, execution schedule writes, checkpoint persistence, and
//! the durable worker.

pub(crate) mod checkpoints;
pub(crate) mod execution_schedules;
pub(crate) mod reconciliation;
pub mod worker;

#[cfg(all(test, not(coverage)))]
mod nats_execution_tests;
