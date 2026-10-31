//! Plaid webhook receiver that publishes verified events to NATS JetStream.

pub mod constants;
pub mod plaid_key_id;
pub mod verification_key;

#[cfg(test)]
mod test_support;

pub use plaid_key_id::PlaidKeyId;
