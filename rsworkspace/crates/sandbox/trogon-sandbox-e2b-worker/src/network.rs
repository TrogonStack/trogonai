//! Translation of the wire `NetworkPolicy` into E2B's sandbox network settings.
//!
//! E2B takes `allow_internet_access` plus a `network` object with `allowOut` (CIDR blocks,
//! bare IPs and domain names; allow entries win over deny entries) and `denyOut` (CIDR
//! blocks only). A user defined policy here is an allow list, so everything not named is
//! denied with `denyOut: ["0.0.0.0/0"]`. Anything E2B cannot carry is refused with
//! `LeaseError::InvalidNetworkPolicy` instead of approximated.

mod constants;
mod e2b_network;
mod internet_access;
mod network_rules;
#[cfg(test)]
mod tests;
mod translate;

pub use e2b_network::E2bNetwork;
pub use internet_access::InternetAccess;
pub use network_rules::NetworkRules;
pub use translate::translate_network_policy;
