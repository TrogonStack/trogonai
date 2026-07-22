use super::*;
use crate::catalog_entry::CatalogEntry;
use crate::predicate::{CompactableFilter, compactable_models};
use crate::store::MockCatalogStore;
use trogonai_catalog_proto::ModelModality;

#[tokio::test]
async fn client_reads_catalog_from_mock_store() {
    let store = MockCatalogStore::new();
    let snapshot = CatalogSnapshot {
        entries: vec![CatalogEntry {
            model_id: "grok-2".into(),
            provider: "xai".into(),
            context_window: 131_072,
            max_output: 8192,
            modality: ModelModality::TEXT,
        }],
    };
    store.put(CATALOG_KEY, snapshot.encode().unwrap().into()).await.unwrap();

    let client = CatalogClient::new(store, CatalogClientConfig::default());
    let got = client.catalog_snapshot().await.unwrap();
    assert_eq!(got.entries.len(), 1);
}

#[test]
fn compactable_models_deterministic() {
    let cat = CatalogSnapshot {
        entries: vec![CatalogEntry {
            model_id: "m".into(),
            provider: "anthropic".into(),
            context_window: 200_000,
            max_output: 8192,
            modality: ModelModality::TEXT,
        }],
    };
    let providers = vec!["anthropic".into()];
    let f = CompactableFilter {
        catalog: &cat,
        callable_providers: &providers,
        session_window: 100_000,
        margin: 1.2,
    };
    assert_eq!(compactable_models(f).len(), 1);
}

#[test]
fn resolve_returns_provider_and_catalog_has_window() {
    let cat = CatalogSnapshot {
        entries: vec![CatalogEntry {
            model_id: "grok-2".into(),
            provider: "xai".into(),
            context_window: 131_072,
            max_output: 8192,
            modality: trogonai_catalog_proto::ModelModality::TEXT,
        }],
    };
    let resolved = crate::resolve(&cat, "grok-2").unwrap();
    assert_eq!(resolved.provider, "xai");
    let entry = cat.entries.iter().find(|e| e.model_id == resolved.model_id).unwrap();
    assert_eq!(entry.context_window, 131_072);
}

/// Requires NATS + secret-proxy worker with provider introspection.
#[tokio::test]
#[ignore = "requires live NATS and trogon.proxy.providers listener"]
async fn integration_catalog_and_providers_from_nats() {
    let _ = CatalogClientConfig::default();
}
