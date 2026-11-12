use super::PeerValueError;
use trogonai_proto::peers::v1alpha1 as proto;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedContentRef {
    locator: String,
    sha256: [u8; 32],
}

impl SealedContentRef {
    pub fn new(reference: proto::ClaimCheck) -> Result<Self, PeerValueError> {
        let locator = reference
            .storage_ref
            .into_option()
            .and_then(|value| value.value)
            .ok_or(PeerValueError::InvalidContentReference)?;
        let locator = super::peer_value_error::parse_token(locator)?;
        let digest = reference
            .digest
            .into_option()
            .ok_or(PeerValueError::InvalidContentReference)?;
        if digest.algorithm.as_deref() != Some("sha256") {
            return Err(PeerValueError::InvalidContentReference);
        }
        let sha256 = digest
            .value
            .ok_or(PeerValueError::InvalidContentReference)?
            .try_into()
            .map_err(|_| PeerValueError::InvalidContentReference)?;
        Ok(Self { locator, sha256 })
    }

    pub fn as_str(&self) -> &str {
        &self.locator
    }

    pub fn integrity_digest(&self) -> &[u8] {
        &self.sha256
    }

    pub fn to_claim_check(&self) -> proto::ClaimCheck {
        proto::ClaimCheck {
            storage_ref: proto::StorageLocator {
                value: Some(self.locator.clone()),
            }
            .into(),
            digest: proto::Digest {
                algorithm: Some("sha256".into()),
                value: Some(self.sha256.to_vec()),
            }
            .into(),
        }
    }
}
