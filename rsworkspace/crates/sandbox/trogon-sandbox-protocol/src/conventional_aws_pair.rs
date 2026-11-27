use trogon_sandbox_contract::{AwsCredentialPairBinding, SecretName};

use crate::UnpairedReason;

/// The pair the conventional AWS environment variables form without being declared, and the members that were left
/// out of it and why.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ConventionalAwsPair {
    pub pair: Option<AwsCredentialPairBinding>,
    pub unpaired: Vec<(SecretName, UnpairedReason)>,
}
