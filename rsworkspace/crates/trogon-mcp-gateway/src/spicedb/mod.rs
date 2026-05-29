//! SpiceDB-backed [`crate::authz::PermissionChecker`] using `spicedb-rs-client`.

mod zedtoken_cache;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use spicedb_rs_client::Client;
use spicedb_rs_client::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsRequestItem, Consistency, ObjectReference, SubjectReference,
    ZedToken, check_bulk_permissions_pair, check_permission_response, consistency,
};
use tokio::sync::Mutex;

pub use zedtoken_cache::{
    CacheKeyParams, ResourceKey, ZedTokenCache, ZedTokenCacheConfig, ZedTokenCacheMetrics,
};

use crate::authz::{AuthzContext, AuthzError, PermissionChecker, ToolsListFilterContext};

fn allowed_spicedb_object_id_char(c: char) -> bool {
    matches!(
        c,
        'a'..='z' | 'A'..='Z' | '0'..='9' | '/' | '_' | '|' | '\\' | '-' | '=' | '+'
    )
}

pub(crate) fn normalize_spicedb_object_token(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if allowed_spicedb_object_id_char(c) {
            out.push(c);
        } else if c.is_ascii_whitespace() {
            out.push('_');
        } else {
            let mut buf = [0u8; 4];
            for byte in c.encode_utf8(&mut buf).bytes() {
                use std::fmt::Write as _;
                write!(&mut out, "_{byte:02x}").unwrap();
            }
        }
    }
    out
}

fn minimize_latency_consistency() -> Consistency {
    Consistency {
        requirement: Some(consistency::Requirement::MinimizeLatency(true)),
    }
}

fn consistency_from_cached_zed_token(maybe_cached: Option<String>) -> Consistency {
    let Some(tok) = maybe_cached.filter(|t| !t.is_empty()) else {
        return minimize_latency_consistency();
    };
    Consistency {
        requirement: Some(consistency::Requirement::AtLeastAsFresh(ZedToken { token: tok })),
    }
}

#[derive(Clone, Debug)]
struct BulkCheckTarget {
    resource_type: String,
    resource_id: String,
    permission: String,
}

#[derive(Clone, Debug)]
pub struct SpicedbCheckerRuntime {
    pub client: Client,
    pub tool_resource_object_type: String,
    pub resource_object_type: String,
    pub subject_object_type: String,
    pub tool_call_permission: String,
    pub resource_read_permission: String,
    pub anonymous_subject_object_id: String,
    pub check_zed_token_cache: Arc<Mutex<Option<String>>>,
}

#[derive(Clone, Debug)]
pub struct SpicedbPermissionChecker {
    inner: SpicedbCheckerRuntime,
    zed_token_cache: Arc<ZedTokenCache>,
}

impl SpicedbPermissionChecker {
    pub fn new(inner: SpicedbCheckerRuntime) -> Self {
        Self {
            inner,
            zed_token_cache: Arc::new(ZedTokenCache::default()),
        }
    }

    pub fn with_zed_token_cache(inner: SpicedbCheckerRuntime, zed_token_cache: Arc<ZedTokenCache>) -> Self {
        Self { inner, zed_token_cache }
    }

    #[must_use]
    pub fn zed_token_cache(&self) -> Arc<ZedTokenCache> {
        self.zed_token_cache.clone()
    }

    fn subject_ref(&self, ctx: &AuthzContext<'_>) -> SubjectReference {
        let raw_subject = ctx
            .caller_sub
            .or(ctx.tenant)
            .unwrap_or(self.inner.anonymous_subject_object_id.as_str());
        let subject_object_id = normalize_spicedb_object_token(raw_subject);
        SubjectReference {
            object: Some(ObjectReference {
                object_type: self.inner.subject_object_type.clone(),
                object_id: subject_object_id,
            }),
            optional_relation: String::new(),
        }
    }

    fn subject_ref_from_parts(
        &self,
        caller_sub: Option<&str>,
        tenant: Option<&str>,
    ) -> SubjectReference {
        let raw_subject = caller_sub
            .or(tenant)
            .unwrap_or(self.inner.anonymous_subject_object_id.as_str());
        let subject_object_id = normalize_spicedb_object_token(raw_subject);
        SubjectReference {
            object: Some(ObjectReference {
                object_type: self.inner.subject_object_type.clone(),
                object_id: subject_object_id,
            }),
            optional_relation: String::new(),
        }
    }

    fn principal_key(&self, caller_sub: Option<&str>, tenant: Option<&str>) -> String {
        let raw_subject = caller_sub
            .or(tenant)
            .unwrap_or(self.inner.anonymous_subject_object_id.as_str());
        ZedTokenCache::principal_key(
            self.inner.subject_object_type.as_str(),
            normalize_spicedb_object_token(raw_subject).as_str(),
        )
    }

    fn cache_key_params(
        &self,
        session_id: &str,
        caller_sub: Option<&str>,
        tenant: Option<&str>,
        permission: &str,
    ) -> CacheKeyParams {
        CacheKeyParams::new(
            session_id,
            self.principal_key(caller_sub, tenant),
            permission.to_string(),
        )
    }

    fn tool_resource_id(server_id: &str, tool_name: &str) -> String {
        let normalized_server = normalize_spicedb_object_token(server_id);
        let normalized_tool = normalize_spicedb_object_token(tool_name);
        format!("{normalized_server}|{normalized_tool}")
    }

    async fn check_permission_bulk_single(
        &self,
        object_type: &str,
        object_id: String,
        permission: &str,
        subject: SubjectReference,
        session_id: Option<&str>,
        caller_sub: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<bool, AuthzError> {
        let resource = ResourceKey::new(object_type, object_id.clone());
        if let Some(session) = session_id.filter(|s| !s.is_empty() && *s != "*") {
            let key_params = self.cache_key_params(session, caller_sub, tenant, permission);
            if let Some(cached_zed) = self.zed_token_cache.get_zed_token(&key_params).await
                && let Some(cached_allowed) = self
                    .zed_token_cache
                    .get_result(&key_params, &resource, Some(cached_zed.as_str()))
                    .await
            {
                return Ok(cached_allowed);
            }
        }

        let cached_zed = if let Some(session) = session_id.filter(|s| !s.is_empty() && *s != "*") {
            let key_params = self.cache_key_params(session, caller_sub, tenant, permission);
            self.zed_token_cache.get_zed_token(&key_params).await
        } else {
            self.inner.check_zed_token_cache.lock().await.clone()
        };

        let target = BulkCheckTarget {
            resource_type: object_type.to_string(),
            resource_id: object_id,
            permission: permission.to_string(),
        };
        let outcomes = self
            .bulk_check_permissions(
                std::slice::from_ref(&target),
                subject,
                cached_zed,
                session_id,
                caller_sub,
                tenant,
            )
            .await?;
        outcomes
            .into_iter()
            .next()
            .map(|(_, allowed)| allowed)
            .ok_or_else(|| AuthzError("SpiceDB CheckBulkPermissions returned no pairs".to_string()))
    }

    async fn bulk_check_permissions(
        &self,
        targets: &[BulkCheckTarget],
        subject: SubjectReference,
        cached_zed_token: Option<String>,
        session_id: Option<&str>,
        caller_sub: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<HashMap<(String, String), bool>, AuthzError> {
        if targets.is_empty() {
            return Ok(HashMap::new());
        }

        let consistency = consistency_from_cached_zed_token(cached_zed_token.clone());
        let items: Vec<CheckBulkPermissionsRequestItem> = targets
            .iter()
            .map(|target| CheckBulkPermissionsRequestItem {
                resource: Some(ObjectReference {
                    object_type: target.resource_type.clone(),
                    object_id: target.resource_id.clone(),
                }),
                permission: target.permission.clone(),
                subject: Some(subject.clone()),
                context: None,
            })
            .collect();

        let request = CheckBulkPermissionsRequest {
            consistency: Some(consistency),
            items,
            with_tracing: false,
        };

        let response = self
            .inner
            .client
            .permissions()
            .check_bulk_permissions(request)
            .await
            .map_err(|status| AuthzError(status.to_string()))?
            .into_inner();

        let checked_at = response.checked_at.and_then(|t| {
            let trimmed = t.token.trim().to_owned();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        });

        if let Some(ref zt) = checked_at {
            if let Some(session) = session_id.filter(|s| !s.is_empty() && *s != "*") {
                let permission = targets[0].permission.as_str();
                let key_params = self.cache_key_params(session, caller_sub, tenant, permission);
                self.zed_token_cache
                    .insert_zed_token(&key_params, zt.clone(), None)
                    .await;
            } else {
                let mut lock = self.inner.check_zed_token_cache.lock().await;
                *lock = Some(zt.clone());
            }
        }

        let mut outcomes = HashMap::with_capacity(targets.len());
        let mut cache_results = Vec::with_capacity(targets.len());

        for pair in response.pairs {
            let Some(item) = pair.request else {
                continue;
            };
            let resource = item.resource.ok_or_else(|| {
                AuthzError("SpiceDB CheckBulkPermissions pair missing resource".to_string())
            })?;
            let key = (resource.object_type.clone(), resource.object_id.clone());
            let allowed = match pair.response {
                Some(check_bulk_permissions_pair::Response::Item(item)) => {
                    item.permissionship == check_permission_response::Permissionship::HasPermission as i32
                }
                Some(check_bulk_permissions_pair::Response::Error(st)) => {
                    return Err(AuthzError(format!(
                        "SpiceDB bulk permission check error: {}",
                        st.message
                    )));
                }
                None => {
                    return Err(AuthzError(
                        "SpiceDB CheckBulkPermissions missing pair inner response".to_string(),
                    ));
                }
            };
            outcomes.insert(key.clone(), allowed);
            cache_results.push((
                ResourceKey::new(resource.object_type, resource.object_id),
                allowed,
            ));
        }

        if let (Some(zt), Some(session)) = (
            checked_at.as_deref(),
            session_id.filter(|s| !s.is_empty() && *s != "*"),
        ) {
            let permission = targets[0].permission.as_str();
            let key_params = self.cache_key_params(session, caller_sub, tenant, permission);
            self.zed_token_cache
                .insert_results(&key_params, zt, &cache_results, None)
                .await;
        }

        Ok(outcomes)
    }

    async fn filter_tool_names_with_cache(
        &self,
        ctx: ToolsListFilterContext<'_>,
        tool_names: &[String],
    ) -> Result<Vec<String>, AuthzError> {
        if tool_names.is_empty() {
            return Ok(Vec::new());
        }

        let key_params = self.cache_key_params(
            ctx.session_id,
            ctx.caller_sub,
            ctx.tenant,
            self.inner.tool_call_permission.as_str(),
        );
        let cached_zed = self.zed_token_cache.get_zed_token(&key_params).await;

        let subject = self.subject_ref_from_parts(ctx.caller_sub, ctx.tenant);
        let mut allowed_tools = Vec::new();
        let mut misses = Vec::new();
        let mut miss_tool_names = Vec::new();

        for tool_name in tool_names {
            let resource_id = Self::tool_resource_id(ctx.server_id, tool_name);
            let resource = ResourceKey::new(
                self.inner.tool_resource_object_type.as_str(),
                resource_id.clone(),
            );
            if let Some(cached_zed) = cached_zed.as_deref()
                && let Some(allowed) = self
                    .zed_token_cache
                    .get_result(&key_params, &resource, Some(cached_zed))
                    .await
            {
                if allowed {
                    allowed_tools.push(tool_name.clone());
                }
                continue;
            }
            misses.push(BulkCheckTarget {
                resource_type: self.inner.tool_resource_object_type.clone(),
                resource_id,
                permission: self.inner.tool_call_permission.clone(),
            });
            miss_tool_names.push(tool_name.clone());
        }

        if misses.is_empty() {
            return Ok(allowed_tools);
        }

        let outcomes = self
            .bulk_check_permissions(
                &misses,
                subject,
                cached_zed,
                Some(ctx.session_id),
                ctx.caller_sub,
                ctx.tenant,
            )
            .await?;

        for (target, tool_name) in misses.iter().zip(miss_tool_names.iter()) {
            let key = (target.resource_type.clone(), target.resource_id.clone());
            if outcomes.get(&key).copied().unwrap_or(false) {
                allowed_tools.push(tool_name.clone());
            }
        }

        Ok(allowed_tools)
    }
}

#[async_trait]
impl PermissionChecker for SpicedbPermissionChecker {
    async fn authorize_mcp_request(&self, ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
        let subject = self.subject_ref(&ctx);
        match ctx.jsonrpc_method {
            "tools/call" => {
                let Some(tool_name) = ctx.tool_name else {
                    return Err(AuthzError(
                        "tools/call requires params.name for SpiceDB authorization".to_string(),
                    ));
                };
                let resource_id = Self::tool_resource_id(ctx.server_id, tool_name);
                self.check_permission_bulk_single(
                    &self.inner.tool_resource_object_type,
                    resource_id,
                    &self.inner.tool_call_permission,
                    subject,
                    ctx.session_id,
                    ctx.caller_sub,
                    ctx.tenant,
                )
                .await
            }
            "resources/read" => {
                let Some(uri) = ctx.resource_uri else {
                    return Err(AuthzError(
                        "resources/read requires params.uri for SpiceDB authorization".to_string(),
                    ));
                };
                let resource_id = normalize_spicedb_object_token(uri);
                self.check_permission_bulk_single(
                    &self.inner.resource_object_type,
                    resource_id,
                    &self.inner.resource_read_permission,
                    subject,
                    ctx.session_id,
                    ctx.caller_sub,
                    ctx.tenant,
                )
                .await
            }
            _ => Ok(true),
        }
    }

    async fn filter_tools_list(
        &self,
        ctx: ToolsListFilterContext<'_>,
        tool_names: &[String],
    ) -> Result<Vec<String>, AuthzError> {
        self.filter_tool_names_with_cache(ctx, tool_names).await
    }

    async fn invalidate_tools_list_cache_for_server(&self, server_id: &str) {
        self.zed_token_cache.invalidate_server(server_id).await;
    }

    async fn invalidate_session_authz_cache(&self, session_id: &str) {
        self.zed_token_cache.invalidate_session(session_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rewrites_disallowed_chars() {
        assert_eq!(
            normalize_spicedb_object_token("my:tool:name"),
            "my_3atool_3aname".to_string()
        );
        assert_eq!(normalize_spicedb_object_token("ok-id_1"), "ok-id_1".to_string());
    }

    #[test]
    fn tool_resource_id_joins_server_and_tool() {
        assert_eq!(
            SpicedbPermissionChecker::tool_resource_id("filesystem", "read_file"),
            "filesystem|read_file"
        );
    }
}
