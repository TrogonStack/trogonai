use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use trogon_std::env::ReadEnv;

use crate::signed_export::{
    DEFAULT_SIGNATURE_MAX_AGE, SignatureVerificationConfig, SignedExportEnvelope, SignedExportError,
    verify_signed_export,
};
use crate::signed_export::{Ed25519PublicKey, OperatorKeyId};

pub const ENV_DISCOVERY_OPERATOR_KEYS: &str = "A2A_DISCOVERY_OPERATOR_KEYS";
pub const ENV_DISCOVERY_SIGNATURE_MAX_AGE_SECS: &str = "A2A_DISCOVERY_SIGNATURE_MAX_AGE_SECS";

pub trait OperatorSignatureGate: Send + Sync {
    fn verify(&self, payload: &[u8], envelope: &SignedExportEnvelope) -> Result<(), SignedExportError>;
}

#[derive(Debug, Default)]
pub struct AllowAllOperatorSignatureGate;

impl OperatorSignatureGate for AllowAllOperatorSignatureGate {
    fn verify(&self, _payload: &[u8], _envelope: &SignedExportEnvelope) -> Result<(), SignedExportError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RealOperatorSignatureGate {
    trusted_keys: BTreeMap<OperatorKeyId, Ed25519PublicKey>,
    verification_config: SignatureVerificationConfig,
}

impl RealOperatorSignatureGate {
    pub fn new(
        trusted_keys: BTreeMap<OperatorKeyId, Ed25519PublicKey>,
        verification_config: SignatureVerificationConfig,
    ) -> Self {
        Self {
            trusted_keys,
            verification_config,
        }
    }

    pub fn trusted_key_count(&self) -> usize {
        self.trusted_keys.len()
    }
}

impl OperatorSignatureGate for RealOperatorSignatureGate {
    fn verify(&self, payload: &[u8], envelope: &SignedExportEnvelope) -> Result<(), SignedExportError> {
        verify_signed_export(&self.trusted_keys, payload, envelope, &self.verification_config)?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum OperatorSignatureGateBuildError {
    Malformed(String),
    InvalidMaxAge(String),
}

impl fmt::Display for OperatorSignatureGateBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "invalid {ENV_DISCOVERY_OPERATOR_KEYS}: {detail}"),
            Self::InvalidMaxAge(raw) => {
                write!(f, "invalid {ENV_DISCOVERY_SIGNATURE_MAX_AGE_SECS}: `{raw}`")
            }
        }
    }
}

impl std::error::Error for OperatorSignatureGateBuildError {}

pub fn signature_max_age_from_env<E: ReadEnv>(env: &E) -> Result<Duration, OperatorSignatureGateBuildError> {
    match env.var(ENV_DISCOVERY_SIGNATURE_MAX_AGE_SECS) {
        Ok(raw) => raw
            .parse::<u64>()
            .map(Duration::from_secs)
            .map_err(|_| OperatorSignatureGateBuildError::InvalidMaxAge(raw)),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_SIGNATURE_MAX_AGE),
        Err(std::env::VarError::NotUnicode(_)) => Err(OperatorSignatureGateBuildError::InvalidMaxAge(
            ENV_DISCOVERY_SIGNATURE_MAX_AGE_SECS.into(),
        )),
    }
}

pub fn parse_operator_keys(raw: &str) -> Result<BTreeMap<OperatorKeyId, Ed25519PublicKey>, OperatorSignatureGateBuildError> {
    if raw.trim().is_empty() {
        return Err(OperatorSignatureGateBuildError::Malformed("empty registry".into()));
    }

    let mut trusted = BTreeMap::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key_id_raw, pubkey_hex) = entry.split_once(':').ok_or_else(|| {
            OperatorSignatureGateBuildError::Malformed(format!("entry `{entry}` must be key_id:hexpubkey"))
        })?;
        let key_id = OperatorKeyId::parse(key_id_raw).map_err(|error| match error {
            SignedExportError::Malformed(detail) => OperatorSignatureGateBuildError::Malformed(detail),
            other => OperatorSignatureGateBuildError::Malformed(other.to_string()),
        })?;
        let public_key = Ed25519PublicKey::parse_hex(pubkey_hex).map_err(|error| match error {
            SignedExportError::Malformed(detail) => OperatorSignatureGateBuildError::Malformed(detail),
            other => OperatorSignatureGateBuildError::Malformed(other.to_string()),
        })?;
        if trusted.insert(key_id, public_key).is_some() {
            return Err(OperatorSignatureGateBuildError::Malformed(format!(
                "duplicate operator key id `{key_id_raw}`"
            )));
        }
    }

    if trusted.is_empty() {
        return Err(OperatorSignatureGateBuildError::Malformed("empty registry".into()));
    }

    Ok(trusted)
}

pub fn try_real_operator_signature_gate_from_env<E: ReadEnv>(
    env: &E,
    now_unix_ms: u64,
) -> Result<RealOperatorSignatureGate, OperatorSignatureGateBuildError> {
    let raw = env.var(ENV_DISCOVERY_OPERATOR_KEYS).map_err(|error| match error {
        std::env::VarError::NotPresent => OperatorSignatureGateBuildError::Malformed(format!("{ENV_DISCOVERY_OPERATOR_KEYS} unset")),
        std::env::VarError::NotUnicode(_) => {
            OperatorSignatureGateBuildError::Malformed(format!("{ENV_DISCOVERY_OPERATOR_KEYS} not unicode"))
        }
    })?;
    let trusted_keys = parse_operator_keys(&raw)?;
    let max_age = signature_max_age_from_env(env)?;
    Ok(RealOperatorSignatureGate::new(
        trusted_keys,
        SignatureVerificationConfig::at_now(now_unix_ms, max_age),
    ))
}

pub enum ResolvedOperatorSignatureGate {
    Real(RealOperatorSignatureGate),
    AllowAll(AllowAllOperatorSignatureGate),
}

impl OperatorSignatureGate for ResolvedOperatorSignatureGate {
    fn verify(&self, payload: &[u8], envelope: &SignedExportEnvelope) -> Result<(), SignedExportError> {
        match self {
            Self::Real(gate) => gate.verify(payload, envelope),
            Self::AllowAll(gate) => gate.verify(payload, envelope),
        }
    }
}

pub fn resolve_operator_signature_gate<E: ReadEnv>(
    env: &E,
    now_unix_ms: u64,
) -> ResolvedOperatorSignatureGate {
    match env.var(ENV_DISCOVERY_OPERATOR_KEYS) {
        Ok(raw) => match parse_operator_keys(&raw) {
            Ok(trusted_keys) => {
                let max_age = signature_max_age_from_env(env).unwrap_or(DEFAULT_SIGNATURE_MAX_AGE);
                ResolvedOperatorSignatureGate::Real(RealOperatorSignatureGate::new(
                    trusted_keys,
                    SignatureVerificationConfig::at_now(now_unix_ms, max_age),
                ))
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "operator signature gate misconfigured — federated imports deny until env is corrected"
                );
                ResolvedOperatorSignatureGate::Real(RealOperatorSignatureGate::new(
                    BTreeMap::new(),
                    SignatureVerificationConfig::at_now(now_unix_ms, DEFAULT_SIGNATURE_MAX_AGE),
                ))
            }
        },
        Err(std::env::VarError::NotPresent) => ResolvedOperatorSignatureGate::AllowAll(AllowAllOperatorSignatureGate),
        Err(std::env::VarError::NotUnicode(_)) => {
            tracing::warn!("operator signature gate env not unicode — federated imports deny");
            ResolvedOperatorSignatureGate::Real(RealOperatorSignatureGate::new(
                BTreeMap::new(),
                SignatureVerificationConfig::at_now(now_unix_ms, DEFAULT_SIGNATURE_MAX_AGE),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use trogon_std::env::InMemoryEnv;

    use super::*;
    use crate::signed_export::sign_discovery_export;

    #[test]
    fn parse_operator_keys_accepts_comma_separated_registry() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let hex = hex::encode(signing_key.verifying_key().to_bytes());
        let raw = format!("ops.prod:{hex},ops.staging:{hex}");

        let trusted = parse_operator_keys(&raw).expect("valid registry");

        assert_eq!(trusted.len(), 2);
        assert!(trusted.contains_key(&OperatorKeyId::parse("ops.prod").unwrap()));
    }

    #[test]
    fn real_gate_rejects_unknown_key() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let key_id = OperatorKeyId::parse("ops.prod").unwrap();
        let payload = b"payload";
        let envelope = sign_discovery_export(&signing_key, key_id, payload, 1);
        let gate = RealOperatorSignatureGate::new(
            BTreeMap::new(),
            SignatureVerificationConfig::at_now(1, DEFAULT_SIGNATURE_MAX_AGE),
        );

        let err = gate.verify(payload, &envelope).expect_err("unknown key");

        assert!(matches!(err, SignedExportError::UnknownKeyId(_)));
    }

    #[test]
    fn resolve_operator_signature_gate_uses_allow_all_when_env_unset() {
        let env = InMemoryEnv::new();
        let gate = resolve_operator_signature_gate(&env, 1);

        let payload = b"payload";
        let envelope = SignedExportEnvelope {
            key_id: OperatorKeyId::parse("ops.prod").unwrap(),
            signed_at_unix_ms: 1,
            payload_sha256: [0; 32],
            signature: [0; 64],
        };

        match gate {
            ResolvedOperatorSignatureGate::AllowAll(allow_all) => {
                allow_all.verify(payload, &envelope).expect("labs bypass");
            }
            ResolvedOperatorSignatureGate::Real(_) => panic!("expected AllowAll when env unset"),
        }
    }

    #[test]
    fn resolve_operator_signature_gate_builds_real_gate_from_env() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let hex = hex::encode(signing_key.verifying_key().to_bytes());
        let env = InMemoryEnv::new();
        env.set(ENV_DISCOVERY_OPERATOR_KEYS, format!("ops.prod:{hex}"));

        let gate = resolve_operator_signature_gate(&env, 1_000);

        match gate {
            ResolvedOperatorSignatureGate::Real(real) => assert_eq!(real.trusted_key_count(), 1),
            ResolvedOperatorSignatureGate::AllowAll(_) => panic!("expected Real gate"),
        }
    }
}
