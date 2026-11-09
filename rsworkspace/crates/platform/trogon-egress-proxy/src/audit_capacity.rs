use crate::connection_limit::ConnectionLimit;

/// A point-in-time count of admitted audit writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditCapacity {
    limit: ConnectionLimit,
    in_flight: usize,
    accepting: bool,
}

impl AuditCapacity {
    pub(crate) fn new(limit: ConnectionLimit, in_flight: usize, accepting: bool) -> Self {
        Self {
            limit,
            in_flight,
            accepting,
        }
    }

    pub fn limit(self) -> ConnectionLimit {
        self.limit
    }

    pub fn in_flight(self) -> usize {
        self.in_flight
    }

    pub fn accepting(self) -> bool {
        self.accepting
    }

    pub fn has_capacity(self) -> bool {
        self.accepting && self.in_flight < self.limit.get()
    }
}
