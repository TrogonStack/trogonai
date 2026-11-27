use trogon_sandbox_contract::LeaseError;

use super::versioned::Versioned;

/// What [`super::Operations::begin`] found.
#[derive(Debug)]
pub enum Begun {
    /// A new operation, queued and holding its lease.
    Created(Versioned),
    /// The id already exists for the same request.
    Existing(Versioned),
    /// The id exists for a different request, or the lease has another operation in flight.
    Conflict(LeaseError),
}
