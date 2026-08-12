pub mod client;
pub mod markers;
pub mod methods;
pub mod server;
pub mod subscriptions;

pub use methods::{McpRole, PeerSubject, method_from_suffix, method_suffix};

#[cfg(test)]
pub use methods::METHOD_TABLE;

#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod tests;
