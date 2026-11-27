use std::fmt;

use crate::naming::{ActorName, Atespace};

/// Which actor a request should reach, and on which port inside it.
///
/// The router has no notion of a per-actor address: it reads this off the
/// `ate-target-actor` header on a CONNECT request. Carrying it as its own type, rather than
/// as a formatted string threaded through call sites, keeps the header format defined in
/// exactly one place.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ActorTarget {
    atespace: Atespace,
    actor: ActorName,
    port: u16,
}

impl ActorTarget {
    pub fn new(atespace: Atespace, actor: ActorName, port: u16) -> Self {
        Self { atespace, actor, port }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The `ate-target-actor` header value that selects this actor at the router.
    pub fn header_value(&self) -> String {
        format!("{}/{}", self.atespace.as_str(), self.actor.as_str())
    }
}

impl fmt::Display for ActorTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.header_value(), self.port)
    }
}
