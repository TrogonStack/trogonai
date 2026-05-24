mod cache;
mod client;
mod config;

#[cfg(test)]
mod tests;

pub use cache::{ImportGateCacheKey, ZedTokenCache, ZedTokenSnapshot};
pub use client::BulkImportPermissionCheck;
pub use config::{
    ENV_SPICEDB_ENDPOINT, ENV_SPICEDB_TOKEN, ENV_SPICEDB_ZEDTOKEN_TTL_SECS, SpiceDbEndpoint, SpiceDbImportGateBuildError,
    SpiceDbToken, ZedTokenTtl,
};

use std::sync::Arc;

use async_trait::async_trait;
use authzed::v1::check_bulk_permissions_pair;
use authzed::v1::check_permission_response::Permissionship;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsRequestItem, Consistency, ObjectReference, SubjectReference,
    ZedToken,
};
use trogon_std::env::ReadEnv;

use crate::agent_id::A2aAgentId;

use super::error::ImportGateError;
use super::gate::ImportGate;
use super::principal::{ImportedAccountName, SpiceDbPrincipal};

use client::LiveBulkImportPermissionClient;
use config::{optional_spicedb_credentials, zed_token_ttl_from_env};

const FEDERATED_AGENT_CARD_RESOURCE_TYPE: &str = "agent_card";
const FEDERATED_IMPORT_PERMISSION: &str = "view";

pub struct SpiceDbImportGate {
    client: Option<Arc<dyn BulkImportPermissionCheck>>,
    zed_token_cache: ZedTokenCache,
}

impl SpiceDbImportGate {
    pub fn deny_only() -> Self {
        Self {
            client: None,
            zed_token_cache: ZedTokenCache::new(ZedTokenTtl::default()),
        }
    }

    pub fn configured(client: Arc<dyn BulkImportPermissionCheck>, zed_token_cache: ZedTokenCache) -> Self {
        Self {
            client: Some(client),
            zed_token_cache,
        }
    }

    pub fn is_configured(&self) -> bool {
        self.client.is_some()
    }

    pub async fn try_from_env<E: ReadEnv>(env: &E) -> Result<Self, SpiceDbImportGateBuildError> {
        let ttl = zed_token_ttl_from_env(env)?;
        let cache = ZedTokenCache::new(ttl);

        let Some((endpoint, token)) = optional_spicedb_credentials(env)? else {
            return Ok(Self::deny_only());
        };

        let live = LiveBulkImportPermissionClient::connect(&endpoint, &token).await?;
        Ok(Self::configured(Arc::new(live), cache))
    }

    async fn check_import(
        &self,
        principal: &SpiceDbPrincipal,
        imported_from: &ImportedAccountName,
        agent_id: &A2aAgentId,
    ) -> Result<bool, ImportGateError> {
        let Some(client) = &self.client else {
            tracing::debug!("SpiceDbImportGate: rejecting import — Authzed client not configured");
            return Ok(false);
        };

        let Some((subject_type, subject_id)) = import_subject(principal) else {
            tracing::warn!(
                imported_from = %imported_from,
                agent_id = %agent_id,
                "SpiceDbImportGate: denying import — principal lacks import-subject mapping"
            );
            return Ok(false);
        };

        let cache_key = ImportGateCacheKey::new(imported_from, agent_id);
        let mut request = CheckBulkPermissionsRequest {
            items: vec![CheckBulkPermissionsRequestItem {
                resource: Some(federated_agent_card_resource(imported_from, agent_id)),
                permission: FEDERATED_IMPORT_PERMISSION.to_owned(),
                subject: Some(SubjectReference {
                    object: Some(ObjectReference {
                        object_type: subject_type,
                        object_id: subject_id,
                    }),
                    optional_relation: String::new(),
                }),
                context: None,
            }],
            consistency: None,
            with_tracing: false,
        };

        if let Some(snapshot) = self.zed_token_cache.get(&cache_key).await {
            request.consistency = Some(Consistency {
                requirement: Some(authzed::v1::consistency::Requirement::AtLeastAsFresh(
                    ZedToken {
                        token: snapshot.token,
                    },
                )),
            });
        }

        let response = match client.check_bulk_permissions(request).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    imported_from = %imported_from,
                    agent_id = %agent_id,
                    error = %error,
                    "SpiceDbImportGate: denying import — Authzed BulkCheckPermissions transport error"
                );
                return Ok(false);
            }
        };

        let allowed = response.pairs.first().is_some_and(pair_is_allowed);

        if allowed
            && let Some(token) = response.checked_at.map(|zed| zed.token)
        {
            self.zed_token_cache.insert(cache_key, token).await;
        }

        Ok(allowed)
    }
}

fn pair_is_allowed(pair: &authzed::v1::CheckBulkPermissionsPair) -> bool {
    match pair.response.as_ref() {
        Some(check_bulk_permissions_pair::Response::Item(item)) => {
            item.permissionship == Permissionship::HasPermission as i32
        }
        Some(check_bulk_permissions_pair::Response::Error(_)) | None => false,
    }
}

impl Default for SpiceDbImportGate {
    fn default() -> Self {
        Self::deny_only()
    }
}

#[async_trait]
impl ImportGate for SpiceDbImportGate {
    async fn permit(
        &self,
        principal: &SpiceDbPrincipal,
        imported_from: &ImportedAccountName,
        agent_id: &A2aAgentId,
    ) -> Result<bool, ImportGateError> {
        self.check_import(principal, imported_from, agent_id).await
    }
}

fn import_subject(principal: &SpiceDbPrincipal) -> Option<(String, String)> {
    if let Some(account) = principal
        .0
        .get("account")
        .or_else(|| principal.0.get("aud"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Some(("account".to_owned(), account.to_owned()));
    }

    principal
        .0
        .get("spicedb_subject")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_subject_reference)
}

fn parse_subject_reference(raw: &str) -> Option<(String, String)> {
    if let Some((object_type, object_id)) = raw.split_once('/')
        && !object_type.is_empty()
        && !object_id.is_empty()
    {
        return Some((object_type.to_owned(), object_id.to_owned()));
    }
    if let Some((object_type, object_id)) = raw.split_once(':')
        && !object_type.is_empty()
        && !object_id.is_empty()
    {
        return Some((object_type.to_owned(), object_id.to_owned()));
    }
    None
}

fn federated_agent_card_resource(
    imported_from: &ImportedAccountName,
    agent_id: &A2aAgentId,
) -> ObjectReference {
    ObjectReference {
        object_type: FEDERATED_AGENT_CARD_RESOURCE_TYPE.to_owned(),
        object_id: format!("{}:{}", imported_from.as_str(), agent_id.as_str()),
    }
}

pub async fn resolve_import_gate<E: ReadEnv>(env: &E) -> SpiceDbImportGate {
    match SpiceDbImportGate::try_from_env(env).await {
        Ok(gate) => gate,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "SpiceDbImportGate: invalid configuration — federated imports deny until env is corrected"
            );
            SpiceDbImportGate::deny_only()
        }
    }
}
