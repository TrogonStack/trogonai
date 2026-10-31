//! Plaid webhook receiver that publishes verified events to NATS JetStream.

pub mod body_digest;
pub mod constants;
pub mod plaid_api_base_url;
pub mod plaid_client_id;
pub mod plaid_delivery_id;
pub mod plaid_environment;
pub mod plaid_key_id;
pub mod plaid_secret;
pub mod signature;
pub mod verification_key;

#[cfg(test)]
mod test_support;

pub use body_digest::PlaidBodyDigest;
pub use plaid_api_base_url::PlaidApiBaseUrl;
pub use plaid_client_id::PlaidClientId;
pub use plaid_delivery_id::PlaidDeliveryId;
pub use plaid_environment::PlaidEnvironment;
pub use plaid_key_id::PlaidKeyId;
pub use plaid_secret::PlaidSecret;
