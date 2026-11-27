use trogon_sandbox_contract::SecretName;

use crate::{RealValue, Sentinel};

/// The real members of a linked AWS credential pair, looked up for one destination.
#[derive(Debug, Clone, Copy)]
pub struct AwsCredentialPair<'a> {
    pub binding: &'a SecretName,
    pub access_key_id_sentinel: &'a Sentinel,
    pub access_key_id: &'a RealValue,
    pub secret_access_key: &'a RealValue,
    pub session_token: Option<&'a RealValue>,
}
