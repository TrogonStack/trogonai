use std::collections::BTreeSet;

use crate::egress_destination::EgressDestination;
use crate::egress_host_name::EgressHostName;
use crate::port::Port;

/// The destinations a subject may open connections to.
///
/// A Session with no permission on its Agent or any ancestor node has no
/// network egress at all, so absence of this binding is denial.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressPermission {
    destinations: BTreeSet<EgressDestination>,
}

impl EgressPermission {
    /// Builds a permission from the destinations it names.
    pub fn new(destinations: impl IntoIterator<Item = EgressDestination>) -> Self {
        Self {
            destinations: destinations.into_iter().collect(),
        }
    }

    /// Returns whether one of the destinations is exactly the requested one.
    pub fn covers(&self, host: &EgressHostName, port: Port) -> bool {
        self.destinations
            .iter()
            .any(|destination| destination.matches(host, port))
    }

    /// Iterates the destinations in sorted order.
    pub fn destinations(&self) -> impl Iterator<Item = &EgressDestination> {
        self.destinations.iter()
    }
}

impl FromIterator<EgressDestination> for EgressPermission {
    fn from_iter<I: IntoIterator<Item = EgressDestination>>(destinations: I) -> Self {
        Self::new(destinations)
    }
}
