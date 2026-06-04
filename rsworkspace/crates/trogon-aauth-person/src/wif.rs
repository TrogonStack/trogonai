//! Workload Identity Federation records.
//!
//! Two record shapes back the WIF token-exchange surface:
//!
//! - [`WorkloadIdentityProviderRecord`] captures **trust**: which external
//!   issuer we accept, which `aud` values bind to us, where signature keys
//!   come from, and the CEL claim mappings that produce derived attributes
//!   from raw verified claims.
//! - [`ServiceAccountMappingRecord`] captures **authorization**: which
//!   derived attributes are allowed to mint a credential for which service
//!   account, with exactly-one-match resolution at exchange time.
//!
//! Records are persisted to JetStream KV buckets using the same
//! `open-or-create` pattern as agents / consents / replay.

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;
use serde::{Deserialize, Serialize};
use trogon_nats::jetstream::is_create_key_value_already_exists;

use crate::store::StoreError;

pub const PROVIDERS_BUCKET: &str = "aauth-wif-providers";
pub const MAPPINGS_BUCKET: &str = "aauth-wif-mappings";

const PROVIDER_VALUE_SIZE: i32 = 65_536;
const MAPPING_VALUE_SIZE: i32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Enabled,
    Disabled,
}

impl LifecycleState {
    #[must_use]
    pub fn is_enabled(self) -> bool {
        matches!(self, LifecycleState::Enabled)
    }
}

/// How a provider's signing keys are obtained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KeySource {
    /// Fetch via OIDC discovery on the issuer URL.
    OidcDiscovery { issuer_url: String },
    /// Operator-uploaded JWKS. New exchanges use this key set immediately.
    InlineJwks { jwks: JwkSet },
}

/// A single CEL claim mapping: produces a derived attribute from raw
/// verified claims under the `assertion.*` namespace.
///
/// Stored as the raw expression source; compilation happens in
/// `a2a-auth-callout::claim_mapping`. Storing source keeps the record
/// language-implementation independent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimMapping {
    pub attribute: String,
    pub expression: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadIdentityProviderRecord {
    pub id: String,
    pub iss: String,
    pub audiences: Vec<String>,
    pub key_source: KeySource,
    #[serde(default)]
    pub claim_mappings: Vec<ClaimMapping>,
    pub lifecycle_state: LifecycleState,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl WorkloadIdentityProviderRecord {
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.lifecycle_state.is_enabled()
    }
}

/// Pattern matched against a single derived attribute.
///
/// `TrailingWildcard("repo:openai/*")` matches anything starting with
/// `"repo:openai/"`. Wildcards are intentionally restricted to a single
/// trailing `*` to keep resolution behavior predictable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MatchPattern {
    Exact { value: String },
    TrailingWildcard { prefix: String },
}

impl MatchPattern {
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        match self {
            MatchPattern::Exact { value } => value == candidate,
            MatchPattern::TrailingWildcard { prefix } => candidate.starts_with(prefix),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAccountMappingRecord {
    pub id: String,
    pub provider_id: String,
    /// Map of derived attribute name → required pattern. Every entry must
    /// match for this mapping to apply.
    pub match_attributes: std::collections::BTreeMap<String, MatchPattern>,
    pub service_account_id: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    pub lifecycle_state: LifecycleState,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl ServiceAccountMappingRecord {
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.lifecycle_state.is_enabled()
    }

    /// Returns true when every entry in `match_attributes` is satisfied by
    /// the corresponding entry in `attributes`. A missing attribute fails.
    pub fn matches(&self, attributes: &std::collections::BTreeMap<String, String>) -> bool {
        for (key, pattern) in &self.match_attributes {
            match attributes.get(key) {
                Some(value) if pattern.matches(value) => continue,
                _ => return false,
            }
        }
        true
    }
}

#[must_use]
pub fn providers_bucket_config() -> kv::Config {
    kv::Config {
        bucket: PROVIDERS_BUCKET.to_owned(),
        history: 1,
        max_value_size: PROVIDER_VALUE_SIZE,
        ..Default::default()
    }
}

#[must_use]
pub fn mappings_bucket_config() -> kv::Config {
    kv::Config {
        bucket: MAPPINGS_BUCKET.to_owned(),
        history: 1,
        max_value_size: MAPPING_VALUE_SIZE,
        ..Default::default()
    }
}

/// Persistence for the two WIF record types.
///
/// Listing pulls every record from the bucket; expected to be cached
/// in front of this trait for hot-path resolution.
#[async_trait]
pub trait WifStore: Send + Sync {
    async fn put_provider(&self, record: WorkloadIdentityProviderRecord) -> Result<(), StoreError>;
    async fn get_provider(&self, id: &str) -> Result<Option<WorkloadIdentityProviderRecord>, StoreError>;
    async fn list_providers(&self) -> Result<Vec<WorkloadIdentityProviderRecord>, StoreError>;

    async fn put_mapping(&self, record: ServiceAccountMappingRecord) -> Result<(), StoreError>;
    async fn get_mapping(&self, id: &str) -> Result<Option<ServiceAccountMappingRecord>, StoreError>;
    async fn list_mappings_for_provider(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ServiceAccountMappingRecord>, StoreError>;
}

#[async_trait]
impl<S: WifStore + ?Sized> WifStore for std::sync::Arc<S> {
    async fn put_provider(&self, record: WorkloadIdentityProviderRecord) -> Result<(), StoreError> {
        (**self).put_provider(record).await
    }
    async fn get_provider(&self, id: &str) -> Result<Option<WorkloadIdentityProviderRecord>, StoreError> {
        (**self).get_provider(id).await
    }
    async fn list_providers(&self) -> Result<Vec<WorkloadIdentityProviderRecord>, StoreError> {
        (**self).list_providers().await
    }
    async fn put_mapping(&self, record: ServiceAccountMappingRecord) -> Result<(), StoreError> {
        (**self).put_mapping(record).await
    }
    async fn get_mapping(&self, id: &str) -> Result<Option<ServiceAccountMappingRecord>, StoreError> {
        (**self).get_mapping(id).await
    }
    async fn list_mappings_for_provider(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ServiceAccountMappingRecord>, StoreError> {
        (**self).list_mappings_for_provider(provider_id).await
    }
}

#[derive(Default)]
pub struct InMemoryWifStore {
    providers: tokio::sync::RwLock<std::collections::HashMap<String, WorkloadIdentityProviderRecord>>,
    mappings: tokio::sync::RwLock<std::collections::HashMap<String, ServiceAccountMappingRecord>>,
}

impl InMemoryWifStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl WifStore for InMemoryWifStore {
    async fn put_provider(&self, record: WorkloadIdentityProviderRecord) -> Result<(), StoreError> {
        self.providers.write().await.insert(record.id.clone(), record);
        Ok(())
    }
    async fn get_provider(&self, id: &str) -> Result<Option<WorkloadIdentityProviderRecord>, StoreError> {
        Ok(self.providers.read().await.get(id).cloned())
    }
    async fn list_providers(&self) -> Result<Vec<WorkloadIdentityProviderRecord>, StoreError> {
        Ok(self.providers.read().await.values().cloned().collect())
    }
    async fn put_mapping(&self, record: ServiceAccountMappingRecord) -> Result<(), StoreError> {
        self.mappings.write().await.insert(record.id.clone(), record);
        Ok(())
    }
    async fn get_mapping(&self, id: &str) -> Result<Option<ServiceAccountMappingRecord>, StoreError> {
        Ok(self.mappings.read().await.get(id).cloned())
    }
    async fn list_mappings_for_provider(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ServiceAccountMappingRecord>, StoreError> {
        Ok(self
            .mappings
            .read()
            .await
            .values()
            .filter(|m| m.provider_id == provider_id)
            .cloned()
            .collect())
    }
}

#[derive(Clone)]
pub struct JetStreamWifStore {
    providers: kv::Store,
    mappings: kv::Store,
}

impl JetStreamWifStore {
    #[must_use]
    pub fn new(providers: kv::Store, mappings: kv::Store) -> Self {
        Self { providers, mappings }
    }

    pub async fn open(js: &jetstream::Context, auto_create: bool) -> Result<Self, StoreError> {
        let providers = open_or_create(js, providers_bucket_config(), PROVIDERS_BUCKET, auto_create).await?;
        let mappings = open_or_create(js, mappings_bucket_config(), MAPPINGS_BUCKET, auto_create).await?;
        Ok(Self::new(providers, mappings))
    }
}

async fn open_or_create(
    js: &jetstream::Context,
    config: kv::Config,
    name: &str,
    auto_create: bool,
) -> Result<kv::Store, StoreError> {
    if auto_create {
        match js.create_key_value(config).await {
            Ok(store) => Ok(store),
            Err(error) if is_create_key_value_already_exists(&error) => js
                .get_key_value(name.to_owned())
                .await
                .map_err(StoreError::BucketOpen),
            Err(error) => Err(StoreError::BucketCreate(error)),
        }
    } else {
        js.get_key_value(name.to_owned())
            .await
            .map_err(StoreError::BucketOpen)
    }
}

async fn collect_kv<T: serde::de::DeserializeOwned>(store: &kv::Store) -> Result<Vec<T>, StoreError> {
    use futures::StreamExt;
    let mut out = Vec::new();
    let mut keys = store
        .keys()
        .await
        .map_err(|e| StoreError::Backend(e.to_string()))?
        .boxed();
    while let Some(key) = keys.next().await {
        let key = key.map_err(|e| StoreError::Backend(e.to_string()))?;
        let Some(entry) = store.entry(&key).await.map_err(StoreError::KvGet)? else {
            continue;
        };
        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            continue;
        }
        let record: T = serde_json::from_slice(&entry.value).map_err(StoreError::Deserialize)?;
        out.push(record);
    }
    Ok(out)
}

#[async_trait]
impl WifStore for JetStreamWifStore {
    async fn put_provider(&self, record: WorkloadIdentityProviderRecord) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(&record).map_err(StoreError::Serialize)?;
        self.providers
            .put(record.id.clone(), bytes.into())
            .await
            .map_err(StoreError::KvPut)?;
        Ok(())
    }

    async fn get_provider(&self, id: &str) -> Result<Option<WorkloadIdentityProviderRecord>, StoreError> {
        let Some(entry) = self.providers.entry(id).await.map_err(StoreError::KvGet)? else {
            return Ok(None);
        };
        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(None);
        }
        let record = serde_json::from_slice(&entry.value).map_err(StoreError::Deserialize)?;
        Ok(Some(record))
    }

    async fn list_providers(&self) -> Result<Vec<WorkloadIdentityProviderRecord>, StoreError> {
        collect_kv(&self.providers).await
    }

    async fn put_mapping(&self, record: ServiceAccountMappingRecord) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(&record).map_err(StoreError::Serialize)?;
        self.mappings
            .put(record.id.clone(), bytes.into())
            .await
            .map_err(StoreError::KvPut)?;
        Ok(())
    }

    async fn get_mapping(&self, id: &str) -> Result<Option<ServiceAccountMappingRecord>, StoreError> {
        let Some(entry) = self.mappings.entry(id).await.map_err(StoreError::KvGet)? else {
            return Ok(None);
        };
        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(None);
        }
        let record = serde_json::from_slice(&entry.value).map_err(StoreError::Deserialize)?;
        Ok(Some(record))
    }

    async fn list_mappings_for_provider(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ServiceAccountMappingRecord>, StoreError> {
        let all: Vec<ServiceAccountMappingRecord> = collect_kv(&self.mappings).await?;
        Ok(all.into_iter().filter(|m| m.provider_id == provider_id).collect())
    }
}

/// Outcome of resolving a service-account mapping against derived attributes.
#[derive(Debug, PartialEq, Eq)]
pub enum MappingResolution {
    Matched(ServiceAccountMappingRecord),
    NoMatch,
    AmbiguousMatch { matched_ids: Vec<String> },
}

/// Exactly-one-match resolver.
///
/// Filters by enabled lifecycle, evaluates `match_attributes` against the
/// supplied derived attributes, and fails closed when 0 or ≥2 mappings
/// match. Mirrors the OpenAI WIF rule.
#[must_use]
pub fn resolve_mapping(
    candidates: &[ServiceAccountMappingRecord],
    attributes: &std::collections::BTreeMap<String, String>,
) -> MappingResolution {
    let mut matched: Vec<&ServiceAccountMappingRecord> = candidates
        .iter()
        .filter(|m| m.is_enabled() && m.matches(attributes))
        .collect();

    match matched.len() {
        0 => MappingResolution::NoMatch,
        1 => MappingResolution::Matched(matched.remove(0).clone()),
        _ => MappingResolution::AmbiguousMatch {
            matched_ids: matched.into_iter().map(|m| m.id.clone()).collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn enabled_provider(id: &str, iss: &str) -> WorkloadIdentityProviderRecord {
        WorkloadIdentityProviderRecord {
            id: id.into(),
            iss: iss.into(),
            audiences: vec!["aud.example".into()],
            key_source: KeySource::OidcDiscovery {
                issuer_url: iss.into(),
            },
            claim_mappings: vec![],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        }
    }

    fn mapping(id: &str, attrs: &[(&str, MatchPattern)]) -> ServiceAccountMappingRecord {
        let mut match_attrs = BTreeMap::new();
        for (k, v) in attrs {
            match_attrs.insert((*k).to_string(), v.clone());
        }
        ServiceAccountMappingRecord {
            id: id.into(),
            provider_id: "wif_p".into(),
            match_attributes: match_attrs,
            service_account_id: "svcacct".into(),
            permissions: vec![],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        }
    }

    fn attrs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn match_pattern_exact() {
        assert!(
            MatchPattern::Exact {
                value: "a".into()
            }
            .matches("a")
        );
        assert!(
            !MatchPattern::Exact {
                value: "a".into()
            }
            .matches("ab")
        );
    }

    #[test]
    fn match_pattern_trailing_wildcard() {
        let p = MatchPattern::TrailingWildcard {
            prefix: "repo:openai/".into(),
        };
        assert!(p.matches("repo:openai/sdk"));
        assert!(p.matches("repo:openai/"));
        assert!(!p.matches("repo:google/x"));
    }

    #[test]
    fn record_matches_requires_every_attribute() {
        let m = mapping(
            "m1",
            &[
                (
                    "openai.subject",
                    MatchPattern::Exact {
                        value: "sub-1".into(),
                    },
                ),
                (
                    "openai.repo",
                    MatchPattern::TrailingWildcard {
                        prefix: "repo:openai/".into(),
                    },
                ),
            ],
        );
        assert!(m.matches(&attrs(&[
            ("openai.subject", "sub-1"),
            ("openai.repo", "repo:openai/sdk")
        ])));
        assert!(!m.matches(&attrs(&[("openai.subject", "sub-1")])));
        assert!(!m.matches(&attrs(&[
            ("openai.subject", "sub-1"),
            ("openai.repo", "repo:google/x")
        ])));
    }

    #[test]
    fn resolve_returns_single_match() {
        let m1 = mapping(
            "m1",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "a".into() },
            )],
        );
        let m2 = mapping(
            "m2",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "b".into() },
            )],
        );
        let resolution = resolve_mapping(&[m1, m2], &attrs(&[("openai.subject", "a")]));
        match resolution {
            MappingResolution::Matched(rec) => assert_eq!(rec.id, "m1"),
            _ => panic!("expected single match"),
        }
    }

    #[test]
    fn resolve_rejects_zero_match() {
        let m = mapping(
            "m1",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "a".into() },
            )],
        );
        let resolution = resolve_mapping(&[m], &attrs(&[("openai.subject", "z")]));
        assert!(matches!(resolution, MappingResolution::NoMatch));
    }

    #[test]
    fn resolve_rejects_ambiguous_match() {
        let m1 = mapping(
            "m1",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "a".into() },
            )],
        );
        let m2 = mapping(
            "m2",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "a".into() },
            )],
        );
        let resolution = resolve_mapping(&[m1, m2], &attrs(&[("openai.subject", "a")]));
        match resolution {
            MappingResolution::AmbiguousMatch { matched_ids } => {
                assert_eq!(matched_ids, vec!["m1", "m2"]);
            }
            _ => panic!("expected ambiguous"),
        }
    }

    #[test]
    fn resolve_skips_disabled() {
        let mut m = mapping(
            "m1",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "a".into() },
            )],
        );
        m.lifecycle_state = LifecycleState::Disabled;
        let resolution = resolve_mapping(&[m], &attrs(&[("openai.subject", "a")]));
        assert!(matches!(resolution, MappingResolution::NoMatch));
    }

    #[tokio::test]
    async fn in_memory_round_trips_provider() {
        let store = InMemoryWifStore::new();
        let p = enabled_provider("wif_p1", "https://iss.example");
        store.put_provider(p.clone()).await.unwrap();
        let got = store.get_provider("wif_p1").await.unwrap();
        assert_eq!(got.as_ref(), Some(&p));
        let listed = store.list_providers().await.unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn in_memory_filters_mappings_by_provider() {
        let store = InMemoryWifStore::new();
        let mut m1 = mapping(
            "m1",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "a".into() },
            )],
        );
        m1.provider_id = "wif_a".into();
        let mut m2 = mapping(
            "m2",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "b".into() },
            )],
        );
        m2.provider_id = "wif_b".into();
        store.put_mapping(m1).await.unwrap();
        store.put_mapping(m2).await.unwrap();

        let a = store.list_mappings_for_provider("wif_a").await.unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].id, "m1");
    }

    #[test]
    fn provider_record_round_trips_json() {
        let p = enabled_provider("wif_p", "https://iss");
        let bytes = serde_json::to_vec(&p).unwrap();
        let back: WorkloadIdentityProviderRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn mapping_record_round_trips_json() {
        let m = mapping(
            "m1",
            &[(
                "openai.subject",
                MatchPattern::Exact { value: "x".into() },
            )],
        );
        let bytes = serde_json::to_vec(&m).unwrap();
        let back: ServiceAccountMappingRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(m, back);
    }
}
