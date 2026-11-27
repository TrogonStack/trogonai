use serde::{Deserialize, Serialize};

use crate::aws_credential_pair_binding_wire::AwsCredentialPairBindingWire;
use crate::{AwsCredentialPairBindingError, SecretName};

/// Links masked bindings into one AWS credential.
///
/// A SigV4 signature is an HMAC keyed by the secret access key, so the stand-in secret the sandbox signs with can only
/// be corrected by re-signing with the real one, and the stand-in access key id in the credential scope is what
/// identifies the pair. Each member is an ordinary whole-value `mask` binding, named here by its secret name.
///
/// The members are distinct, and construction and deserialization both reject a name used twice. Whether the named
/// bindings exist and fit is checked when the pair meets its bindings, in
/// [`AwsCredentialPairs`](crate::AwsCredentialPairs).
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "AwsCredentialPairBindingWire", into = "AwsCredentialPairBindingWire")]
pub struct AwsCredentialPairBinding {
    access_key_id: SecretName,
    secret_access_key: SecretName,
    session_token: Option<SecretName>,
}

impl AwsCredentialPairBinding {
    pub fn new(
        access_key_id: SecretName,
        secret_access_key: SecretName,
        session_token: Option<SecretName>,
    ) -> Result<Self, AwsCredentialPairBindingError> {
        let pair = Self {
            access_key_id,
            secret_access_key,
            session_token,
        };
        let members: Vec<&SecretName> = pair.members().collect();
        for (index, member) in members.iter().enumerate() {
            if members[..index].contains(member) {
                return Err(AwsCredentialPairBindingError::DuplicateMember((*member).clone()));
            }
        }
        Ok(pair)
    }

    pub fn access_key_id(&self) -> &SecretName {
        &self.access_key_id
    }

    pub fn secret_access_key(&self) -> &SecretName {
        &self.secret_access_key
    }

    pub fn session_token(&self) -> Option<&SecretName> {
        self.session_token.as_ref()
    }

    /// The member names in order: access key id, secret access key, then the session token when present.
    pub fn members(&self) -> impl Iterator<Item = &SecretName> {
        [&self.access_key_id, &self.secret_access_key]
            .into_iter()
            .chain(self.session_token.as_ref())
    }
}

impl TryFrom<AwsCredentialPairBindingWire> for AwsCredentialPairBinding {
    type Error = AwsCredentialPairBindingError;

    fn try_from(wire: AwsCredentialPairBindingWire) -> Result<Self, Self::Error> {
        Self::new(wire.access_key_id, wire.secret_access_key, wire.session_token)
    }
}

impl From<AwsCredentialPairBinding> for AwsCredentialPairBindingWire {
    fn from(pair: AwsCredentialPairBinding) -> Self {
        Self {
            access_key_id: pair.access_key_id,
            secret_access_key: pair.secret_access_key,
            session_token: pair.session_token,
        }
    }
}
