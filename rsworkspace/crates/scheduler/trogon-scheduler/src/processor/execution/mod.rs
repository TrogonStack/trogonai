//! The execution-schedule processor.
//!
//! This processor consumes persisted `v1::ScheduleEvent` records and reconciles
//! enabled `At`/`Every`/`Cron` definitions into NATS execution schedules. It
//! owns everything for that one concern: recorded event decoding, pure
//! reconciliation rules, execution schedule writes, checkpoint persistence, and
//! the durable worker.
#![cfg_attr(
    dylint_lib = "trogon_lints",
    expect(
        acyclic_modules,
        reason = "reconciliation resumes from the checkpoint it last wrote and a checkpoint records the reconciliation position it was taken at"
    )
)]

pub(crate) mod checkpoints;
pub(crate) mod execution_schedules;
pub(crate) mod reconciliation;
pub mod wakeup;
pub mod worker;

#[cfg(test)]
mod nats_execution_tests;
