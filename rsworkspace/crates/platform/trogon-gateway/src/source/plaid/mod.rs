//! Plaid webhook receiver that publishes verified events to NATS JetStream.

pub mod body_digest;
pub mod constants;
pub mod plaid_delivery_id;
pub mod plaid_key_id;
pub mod signature;
pub mod verification_key;

#[cfg(test)]
mod test_support;

pub use body_digest::PlaidBodyDigest;
pub use plaid_delivery_id::PlaidDeliveryId;
pub use plaid_key_id::PlaidKeyId;
