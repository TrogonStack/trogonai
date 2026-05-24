use std::collections::HashSet;
use std::fmt;

use a2a_nats::catalog::import_gate::{ImportGate, ImportGateError, ImportedAccountName, SpiceDbPrincipal};
use a2a_nats::catalog::store::{CatalogStoreError, KvCatalogStore};
use a2a_nats::agent_id::A2aAgentId;

use crate::operator_signature_gate::OperatorSignatureGate;
use crate::signed_export::{SignedExportEnvelope, SignedExportError};

#[derive(Debug)]
pub enum FederatedImportError {
    Signature(SignedExportError),
    ImportGate(ImportGateError),
    MissingExportBinding(ImportedAccountName),
}

impl fmt::Display for FederatedImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signature(error) => write!(f, "{error}"),
            Self::ImportGate(error) => write!(f, "{error}"),
            Self::MissingExportBinding(account) => {
                write!(f, "missing signed export binding for imported account `{account}`")
            }
        }
    }
}

impl std::error::Error for FederatedImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Signature(error) => Some(error),
            Self::ImportGate(error) => Some(error),
            Self::MissingExportBinding(_) => None,
        }
    }
}

impl From<SignedExportError> for FederatedImportError {
    fn from(error: SignedExportError) -> Self {
        Self::Signature(error)
    }
}

impl From<ImportGateError> for FederatedImportError {
    fn from(error: ImportGateError) -> Self {
        Self::ImportGate(error)
    }
}

#[derive(Debug, Clone)]
pub struct FederatedExportBinding {
    pub payload: Vec<u8>,
    pub envelope: SignedExportEnvelope,
}

pub async fn permit_federated_import<G, S>(
    signature_gate: &S,
    import_gate: &G,
    principal: &SpiceDbPrincipal,
    imported_from: &ImportedAccountName,
    agent_id: &A2aAgentId,
    export_binding: &FederatedExportBinding,
) -> Result<bool, FederatedImportError>
where
    G: ImportGate + Sync,
    S: OperatorSignatureGate + Sync,
{
    signature_gate.verify(&export_binding.payload, &export_binding.envelope)?;
    import_gate
        .permit(principal, imported_from, agent_id)
        .await
        .map_err(FederatedImportError::from)
}

pub async fn list_cards_federated_gated<K, G, S, F, B>(
    store: &KvCatalogStore<K>,
    signature_gate: &S,
    import_gate: &G,
    principal: &SpiceDbPrincipal,
    imported_source: F,
    export_binding: B,
) -> Result<Vec<a2a_types::AgentCard>, CatalogStoreError>
where
    K: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + trogon_nats::jetstream::JetStreamKvKeys
        + Send
        + Sync
        + Clone
        + 'static,
    G: ImportGate + Sync,
    S: OperatorSignatureGate + Sync,
    F: Fn(&A2aAgentId, &a2a_types::AgentCard) -> Option<ImportedAccountName> + Send + Sync,
    B: Fn(&ImportedAccountName) -> Option<FederatedExportBinding> + Send + Sync,
{
    let pairs = store.list_cards().await?;
    let mut verified_accounts: HashSet<String> = HashSet::new();
    let mut gated = Vec::new();

    for (agent_id, card) in pairs {
        match imported_source(&agent_id, &card) {
            None => gated.push(card),
            Some(imported_from) => {
                let account_key = imported_from.as_str().to_owned();
                if !verified_accounts.contains(&account_key) {
                    let binding = export_binding(&imported_from).ok_or_else(|| {
                        CatalogStoreError::ImportGate(ImportGateError::Gateway(format!(
                            "missing signed export binding for imported account `{imported_from}`"
                        )))
                    })?;
                    signature_gate
                        .verify(&binding.payload, &binding.envelope)
                        .map_err(|error| CatalogStoreError::ImportGate(ImportGateError::Gateway(error.to_string())))?;
                    verified_accounts.insert(account_key);
                }

                if import_gate
                    .permit(principal, &imported_from, &agent_id)
                    .await
                    .map_err(CatalogStoreError::ImportGate)?
                {
                    let value = serde_json::to_value(&card).map_err(CatalogStoreError::Serialize)?;
                    if a2a_pack::accept_agent_card_on_read(&value, a2a_pack::AgentCardSource::FederatedImport) {
                        gated.push(card);
                    }
                }
            }
        }
    }

    Ok(gated)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use a2a_nats::catalog::{AllowAllImportGate, ImportGate, ImportGateError, ImportedAccountName, SpiceDbPrincipal};
    use async_trait::async_trait;
    use bytes::Bytes;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

    use super::*;
    use crate::operator_signature_gate::{
        AllowAllOperatorSignatureGate, RealOperatorSignatureGate, resolve_operator_signature_gate,
        ENV_DISCOVERY_OPERATOR_KEYS,
    };
    use crate::signed_export::{
        DEFAULT_SIGNATURE_MAX_AGE, Ed25519PublicKey, OperatorKeyId, SignatureVerificationConfig,
        SignedExportEnvelope, sign_discovery_export,
    };

    fn minimal_card(display_name: &str) -> a2a_types::AgentCard {
        a2a_types::AgentCard {
            name: display_name.to_string(),
            supported_interfaces: vec![a2a_types::AgentInterface {
                url: "https://example.com/a2a".to_string(),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "0.2.0".to_string(),
                tenant: Default::default(),
            }],
            ..Default::default()
        }
    }

    struct DenyAllImportGate;

    #[async_trait]
    impl ImportGate for DenyAllImportGate {
        async fn permit(
            &self,
            _principal: &SpiceDbPrincipal,
            _imported_from: &ImportedAccountName,
            _agent_id: &A2aAgentId,
        ) -> Result<bool, ImportGateError> {
            Ok(false)
        }
    }

    fn signed_binding(signing_key: &SigningKey, key_id: &OperatorKeyId, payload: &[u8], now: u64) -> FederatedExportBinding {
        FederatedExportBinding {
            payload: payload.to_vec(),
            envelope: sign_discovery_export(signing_key, key_id.clone(), payload, now),
        }
    }

    #[tokio::test]
    async fn list_cards_federated_gated_real_signature_gate_then_allow_all_import() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let key_id = OperatorKeyId::parse("ops.prod").unwrap();
        let mut trusted = BTreeMap::new();
        trusted.insert(
            key_id.clone(),
            Ed25519PublicKey::from_bytes(signing_key.verifying_key().to_bytes()),
        );
        let signature_gate = RealOperatorSignatureGate::new(
            trusted,
            SignatureVerificationConfig::at_now(1_000, DEFAULT_SIGNATURE_MAX_AGE),
        );
        let import_gate = AllowAllImportGate;
        let payload = br#"{"subject":"a2a.discover.>"}"#;
        let binding = signed_binding(&signing_key, &key_id, payload, 1_000);

        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["peer-agent".into()]));
        let federated = minimal_card("federated");
        let federated_bytes = Bytes::from(serde_json::to_vec(&federated).unwrap());
        kv.enqueue_get_some(federated_bytes.clone());
        kv.enqueue_get_some(federated_bytes);

        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");

        let gated = list_cards_federated_gated(
            &store,
            &signature_gate,
            &import_gate,
            &principal,
            |_id, _card| Some(ImportedAccountName::new("peer-account")),
            |_imported| Some(binding.clone()),
        )
        .await
        .expect("signed import passes");

        assert_eq!(gated.len(), 1);
        assert_eq!(gated[0].name, "federated");
    }

    #[tokio::test]
    async fn list_cards_federated_gated_allow_all_signature_gate_keeps_lab_path_open() {
        let signature_gate = AllowAllOperatorSignatureGate;
        let import_gate = AllowAllImportGate;

        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["peer-agent".into()]));
        let federated = minimal_card("federated");
        let federated_bytes = Bytes::from(serde_json::to_vec(&federated).unwrap());
        kv.enqueue_get_some(federated_bytes.clone());
        kv.enqueue_get_some(federated_bytes);

        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");

        let gated = list_cards_federated_gated(
            &store,
            &signature_gate,
            &import_gate,
            &principal,
            |_id, _card| Some(ImportedAccountName::new("peer-account")),
            |_imported| {
                Some(FederatedExportBinding {
                    payload: b"unsigned-lab-payload".to_vec(),
                    envelope: SignedExportEnvelope {
                        key_id: OperatorKeyId::parse("lab").unwrap(),
                        signed_at_unix_ms: 1,
                        payload_sha256: [0; 32],
                        signature: [0; 64],
                    },
                })
            },
        )
        .await
        .expect("AllowAll signature gate keeps import path open");

        assert_eq!(gated.len(), 1);
    }

    #[tokio::test]
    async fn list_cards_federated_gated_rejects_invalid_signature_before_spicedb() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let key_id = OperatorKeyId::parse("ops.prod").unwrap();
        let signature_gate = RealOperatorSignatureGate::new(
            BTreeMap::new(),
            SignatureVerificationConfig::at_now(1_000, DEFAULT_SIGNATURE_MAX_AGE),
        );
        let import_gate = AllowAllImportGate;
        let payload = br#"{"subject":"a2a.discover.>"}"#;
        let binding = signed_binding(&signing_key, &key_id, payload, 1_000);

        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["peer-agent".into()]));
        let federated = minimal_card("federated");
        let federated_bytes = Bytes::from(serde_json::to_vec(&federated).unwrap());
        kv.enqueue_get_some(federated_bytes);

        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");

        let err = list_cards_federated_gated(
            &store,
            &signature_gate,
            &import_gate,
            &principal,
            |_id, _card| Some(ImportedAccountName::new("peer-account")),
            |_imported| Some(binding.clone()),
        )
        .await
        .expect_err("invalid signature short-circuits before SpiceDB");

        assert!(matches!(err, CatalogStoreError::ImportGate(_)));
    }

    #[tokio::test]
    async fn resolve_operator_signature_gate_wires_through_import_path() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let key_id = OperatorKeyId::parse("ops.prod").unwrap();
        let hex = hex::encode(signing_key.verifying_key().to_bytes());
        let env = trogon_std::env::InMemoryEnv::new();
        env.set(ENV_DISCOVERY_OPERATOR_KEYS, format!("ops.prod:{hex}"));

        let signature_gate = resolve_operator_signature_gate(&env, 1_000);
        let import_gate = DenyAllImportGate;
        let payload = br#"{"subject":"a2a.discover.>"}"#;
        let binding = signed_binding(&signing_key, &key_id, payload, 1_000);

        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["peer-agent".into(), "local-agent".into()]));
        let federated = minimal_card("federated");
        let local = minimal_card("local");
        let federated_bytes = Bytes::from(serde_json::to_vec(&federated).unwrap());
        let local_bytes = Bytes::from(serde_json::to_vec(&local).unwrap());
        kv.enqueue_get_some(local_bytes.clone());
        kv.enqueue_get_some(federated_bytes.clone());
        kv.enqueue_get_some(local_bytes);
        kv.enqueue_get_some(federated_bytes);

        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");

        let gated = list_cards_federated_gated(
            &store,
            &signature_gate,
            &import_gate,
            &principal,
            |id, _card| {
                if id.as_str() == "local-agent" {
                    None
                } else {
                    Some(ImportedAccountName::new("peer-account"))
                }
            },
            |_imported| Some(binding.clone()),
        )
        .await
        .expect("signature verified even when SpiceDB denies federated card");

        assert_eq!(gated.len(), 1);
        assert_eq!(gated[0].name, "local");
    }
}
