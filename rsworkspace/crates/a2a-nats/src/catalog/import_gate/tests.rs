use async_trait::async_trait;

use crate::agent_id::A2aAgentId;

use super::allow_all::AllowAllImportGate;
use super::error::ImportGateError;
use super::gate::ImportGate;
use super::principal::{ImportedAccountName, SpiceDbPrincipal};

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

#[tokio::test]
async fn allow_all_permits_everything() {
    let gate = AllowAllImportGate;
    let p = SpiceDbPrincipal::new("user:alice");
    let imported = ImportedAccountName::new("peer-acct");
    let aid = A2aAgentId::new("bot").unwrap();
    assert!(
        gate
            .permit(&p, &imported, &aid)
            .await
            .expect("AllowAll gate must succeed")
    );
}

#[tokio::test]
async fn deny_all_denies_when_import_source_claimed() {
    let gate = DenyAllImportGate;
    let p = SpiceDbPrincipal::new("user:alice");
    let imported = ImportedAccountName::new("peer-acct");
    let aid = A2aAgentId::new("bot").unwrap();
    assert!(!gate.permit(&p, &imported, &aid).await.expect("must not error"));
}

#[cfg(test)]
mod catalog_list_tests {
    use bytes::Bytes;
    use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

    use crate::agent_id::A2aAgentId;
    use crate::catalog::store::{CatalogStoreError, KvCatalogStore};
    use crate::catalog::{
        AllowAllImportGate, ImportGate, ImportGateError, ImportedAccountName, SpiceDbPrincipal,
    };

    struct CatalogDenyAllImportGate;

    #[async_trait::async_trait]
    impl ImportGate for CatalogDenyAllImportGate {
        async fn permit(
            &self,
            _principal: &SpiceDbPrincipal,
            _imported_from: &ImportedAccountName,
            _agent_id: &A2aAgentId,
        ) -> Result<bool, ImportGateError> {
            Ok(false)
        }
    }

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

    #[tokio::test]
    async fn list_cards_gated_allow_all_matches_ungated_names() {
        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["beta-agent".into(), "alpha-agent".into()]));

        let alpha = minimal_card("alpha-card");
        let beta = minimal_card("beta-card");
        let alpha_bytes = Bytes::from(serde_json::to_vec(&alpha).unwrap());
        let beta_bytes = Bytes::from(serde_json::to_vec(&beta).unwrap());
        kv.enqueue_get_some(alpha_bytes.clone());
        kv.enqueue_get_some(beta_bytes.clone());
        kv.enqueue_get_some(alpha_bytes);
        kv.enqueue_get_some(beta_bytes);

        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");

        let ungated = store.list_cards().await.expect("fixture KV list");
        let gated = store
            .list_cards_gated(&AllowAllImportGate, &principal, |_id, _card| None)
            .await
            .expect("AllowAll gated list");

        let mut un_sorted: Vec<_> = ungated.iter().map(|(_, card)| card.name.as_str()).collect();
        let mut gated_sorted = gated.iter().map(|card| card.name.as_str()).collect::<Vec<_>>();

        un_sorted.sort_unstable();
        gated_sorted.sort_unstable();
        assert_eq!(ungated.len(), gated.len());
        assert_eq!(un_sorted, gated_sorted);
    }

    #[tokio::test]
    async fn list_cards_gated_deny_all_drops_cards_with_claimed_cross_account_import() {
        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["peer-agent".into(), "tenant-local".into()]));

        let tenant_local = minimal_card("tenant-local");
        let federated_import = minimal_card("federated");
        let federated_bytes = Bytes::from(serde_json::to_vec(&federated_import).unwrap());
        let tenant_local_bytes = Bytes::from(serde_json::to_vec(&tenant_local).unwrap());

        kv.enqueue_get_some(federated_bytes.clone());
        kv.enqueue_get_some(tenant_local_bytes.clone());
        kv.enqueue_get_some(federated_bytes);
        kv.enqueue_get_some(tenant_local_bytes);

        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");

        let gated = store
            .list_cards_gated(&CatalogDenyAllImportGate, &principal, |id, _card| {
                if id.as_str() == "tenant-local" {
                    None
                } else {
                    Some(ImportedAccountName::new("peer-account"))
                }
            })
            .await
            .expect("DenyAll gated list succeeds");

        assert_eq!(gated.len(), 1);
        assert_eq!(gated[0].name, "tenant-local");
        assert_eq!(store.list_cards().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn list_cards_reports_invalid_catalog_key_segments() {
        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["bad.segment".into()]));

        let store = KvCatalogStore::new(kv);
        let err = store.list_cards().await.expect_err("invalid KV key must surface");
        assert!(matches!(err, CatalogStoreError::Kv(_)));
    }

    #[test]
    fn federated_import_batch_drops_invalid_cards() {
        use a2a_pack::{AgentCardSource, filter_agent_cards_on_read};
        use serde_json::json;

        let good = json!({
            "name": "federated",
            "supportedInterfaces": [{
                "url": "https://example.com/a2a",
                "protocolBinding": "JSONRPC",
                "protocolVersion": "0.2.0"
            }]
        });
        let bad = json!({ "name": "" });
        let filtered = filter_agent_cards_on_read(vec![good.clone(), bad, good], AgentCardSource::FederatedImport);
        assert_eq!(filtered.len(), 2);
    }
}
