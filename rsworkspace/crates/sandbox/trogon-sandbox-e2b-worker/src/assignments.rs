//! Durable ownership of each targeted lease attempt: which tenant it belongs to and which native sandbox it holds.

mod assignment;
mod assignments_error;
mod constants;
mod recovery;
mod sandbox_id;
mod store;
mod stored;
#[cfg(test)]
mod tests;

pub(crate) use assignment::Assignment;
pub use assignments_error::AssignmentsError;
pub(crate) use recovery::Recovery;
pub(crate) use sandbox_id::validate_sandbox_id;
pub(crate) use store::Assignments;
