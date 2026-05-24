use serde::{Deserialize, Serialize};

use super::digest::Sha256Digest;
use super::error::SignatureVerificationError;
use super::signature::Ed25519Signature;
use crate::skill_id::SkillId;

pub const SIGNED_BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedBundleManifest {
    pub version: u32,
    pub skill_id: String,
    pub manifest_sha256: String,
    pub wasm_sha256: String,
    pub signature: String,
}

impl SignedBundleManifest {
    pub fn new(
        skill_id: &SkillId,
        manifest_digest: Sha256Digest,
        wasm_digest: Sha256Digest,
        signature: Ed25519Signature,
    ) -> Self {
        Self {
            version: SIGNED_BUNDLE_VERSION,
            skill_id: skill_id.as_str().to_owned(),
            manifest_sha256: manifest_digest.to_hex(),
            wasm_sha256: wasm_digest.to_hex(),
            signature: signature.to_string(),
        }
    }

    pub fn parse_json(raw: &[u8], skill_id: &SkillId) -> Result<Self, SignatureVerificationError> {
        serde_json::from_slice(raw).map_err(|err| SignatureVerificationError::MalformedSignatureFile {
            skill_id: skill_id.to_string(),
            detail: err.to_string(),
        })
    }

    pub fn manifest_digest(&self, skill_id: &SkillId) -> Result<Sha256Digest, SignatureVerificationError> {
        Sha256Digest::from_hex(&self.manifest_sha256).map_err(|err| SignatureVerificationError::MalformedSignatureFile {
            skill_id: skill_id.to_string(),
            detail: format!("manifest_sha256: {err}"),
        })
    }

    pub fn wasm_digest(&self, skill_id: &SkillId) -> Result<Sha256Digest, SignatureVerificationError> {
        Sha256Digest::from_hex(&self.wasm_sha256).map_err(|err| SignatureVerificationError::MalformedSignatureFile {
            skill_id: skill_id.to_string(),
            detail: format!("wasm_sha256: {err}"),
        })
    }

    pub fn signature_bytes(&self, skill_id: &SkillId) -> Result<Ed25519Signature, SignatureVerificationError> {
        Ed25519Signature::from_hex(&self.signature).map_err(|err| match err {
            SignatureVerificationError::MalformedSignatureFile { detail, .. } => {
                SignatureVerificationError::MalformedSignatureFile {
                    skill_id: skill_id.to_string(),
                    detail,
                }
            }
            other => other,
        })
    }
}
