use trogon_sandbox_contract::LeaseError;

/// How a finite deadline ended, or that it has not yet.
#[derive(Debug, Clone)]
pub(super) enum Terminal {
    Pending,
    Natural(i32),
    TimedOut,
    Uncertain(LeaseError),
}
