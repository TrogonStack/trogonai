//! SpiceDB-backed [`crate::authz::PermissionChecker`] using `spicedb-rs-client`.

use std::sync::Arc;

use async_trait::async_trait;
use spicedb_rs_client::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsRequestItem, Consistency,
    ObjectReference, SubjectReference, ZedToken, check_bulk_permissions_pair,
    check_permission_response, consistency,
};
use spicedb_rs_client::Client;
use tokio::sync::Mutex;

use crate::authz::{AuthzContext, AuthzError, PermissionChecker};

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
        requirement: Some(consistency::Requirement::AtLeastAsFresh(ZedToken {
            token: tok,
        })),
    }
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
}

impl SpicedbPermissionChecker {
    pub fn new(inner: SpicedbCheckerRuntime) -> Self {
        Self { inner }
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

    async fn check_permission_bulk_single(
        &self,
        object_type: &str,
        object_id: String,
        permission: &str,
        subject: SubjectReference,
    ) -> Result<bool, AuthzError> {
        let cached = self.inner.check_zed_token_cache.lock().await.clone();
        let consistency = consistency_from_cached_zed_token(cached);

        let request = CheckBulkPermissionsRequest {
            consistency: Some(consistency),
            items: vec![CheckBulkPermissionsRequestItem {
                resource: Some(ObjectReference {
                    object_type: object_type.to_string(),
                    object_id,
                }),
                permission: permission.to_string(),
                subject: Some(subject),
                context: None,
            }],
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

        if let Some(zt) = response.checked_at.and_then(|t| {
            let trimmed = t.token.trim().to_owned();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }) {
            let mut lock = self.inner.check_zed_token_cache.lock().await;
            *lock = Some(zt);
        }

        let pair = response
            .pairs
            .into_iter()
            .next()
            .ok_or_else(|| AuthzError("SpiceDB CheckBulkPermissions returned no pairs".to_string()))?;

        match pair.response {
            Some(check_bulk_permissions_pair::Response::Item(item)) => Ok(
                item.permissionship
                    == check_permission_response::Permissionship::HasPermission as i32,
            ),
            Some(check_bulk_permissions_pair::Response::Error(st)) => Err(AuthzError(format!(
                "SpiceDB bulk permission check error: {}",
                st.message
            ))),
            None => Err(AuthzError(
                "SpiceDB CheckBulkPermissions missing pair inner response".to_string(),
            )),
        }
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
                let normalized_server = normalize_spicedb_object_token(ctx.server_id);
                let normalized_tool = normalize_spicedb_object_token(tool_name);
                let resource_id = format!("{normalized_server}|{normalized_tool}");
                self.check_permission_bulk_single(
                    &self.inner.tool_resource_object_type,
                    resource_id,
                    &self.inner.tool_call_permission,
                    subject,
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
                )
                .await
            }
            _ => Ok(true),
        }
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
}
