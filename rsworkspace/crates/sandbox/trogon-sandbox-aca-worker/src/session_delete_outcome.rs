/// How a session deletion ended.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SessionDeleteOutcome {
    /// The service confirmed it deleted the session.
    Deleted,
    /// The service has no such session.
    Absent,
}
