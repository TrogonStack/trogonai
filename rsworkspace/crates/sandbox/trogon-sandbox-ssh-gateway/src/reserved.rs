/// Outcome of binding a session id to a lease.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Reserved {
    New,
    Existing,
}
