//! Provider worker conformance suite.
//!
//! The checks talk to a worker over the real provider subjects, not through the broker, so one suite covers any
//! worker that answers them. Point a [`Target`] at a NATS server a worker is already connected to and call
//! [`assert_conforms`].
//!
//! A worker that runs with `SANDBOX_DISPATCH_VERIFY=require` refuses unsigned dispatches. Set [`Target::signer`]
//! (see [`signer_from_env`]) and the suite signs every request the way the broker does, with the same key
//! variables (`SANDBOX_DISPATCH_SIGNING_KEY_FILE` or `SANDBOX_DISPATCH_SIGNING_KEY`).
//!
//! Where real workers legitimately differ (a missing file is `not_found` on one and a backend denial on another)
//! the checks accept either typed answer, but never a success.

mod assignment_fault;
mod check_failure;
mod check_fn;
mod command_checks;
mod conformance_grant;
mod constants;
mod egress_checks;
mod encoding;
mod exec_model;
mod file_verb_checks;
mod grant_ledger;
mod held;
mod ids;
mod js_checks;
mod lease_checks;
mod native_cleanup_authority;
mod not_conformant;
mod outcomes;
mod process_checks;
mod run_fields;
mod selected_worker;
mod signer_from_env;
mod suite;
mod suite_failure;
mod target;

pub use assignment_fault::AssignmentFaultError;
pub use check_failure::CheckFailureError;
pub use check_fn::{CheckFn, CheckFuture};
pub use conformance_grant::ConformanceGrant;
pub use exec_model::ExecModel;
pub use grant_ledger::GrantLedger;
pub use native_cleanup_authority::NativeCleanupAuthority;
pub use not_conformant::NotConformantError;
pub use signer_from_env::signer_from_env;
pub use suite::{assert_conforms, checks, checks_for, checks_for_target, run_suite};
pub use suite_failure::SuiteFailureError;
pub use target::Target;
