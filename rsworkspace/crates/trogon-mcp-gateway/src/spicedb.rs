//! SpiceDB-backed [`crate::authz::PermissionChecker`] using `spicedb-rs-client`.

use async_trait::async_trait;
use spicedb_rs_client::v1::{
    CheckPermissionRequest, Consistency, ObjectReference, SubjectReference,
    check_permission_response, consistency,
};
use spicedb_rs_client::Client;

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

#[derive(Clone, Debug)]
pub struct SpicedbCheckerRuntime {
    pub client: Client,
    pub tool_resource_object_type: String,
    pub resource_object_type: String,
    pub subject_object_type: String,
    pub tool_call_permission: String,
    pub resource_read_permission: String,
    pub anonymous_subject_object_id: String,
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

    async fn check_permission_inner(
        &self,
        object_type: &str,
        object_id: String,
        permission: &str,
        subject: SubjectReference,
    ) -> Result<bool, AuthzError> {
        let request = CheckPermissionRequest {
            consistency: Some(minimize_latency_consistency()),
            resource: Some(ObjectReference {
                object_type: object_type.to_string(),
                object_id,
            }),
            permission: permission.to_string(),
            subject: Some(subject),
            ..Default::default()
        };

        let response = self
            .inner
            .client
            .permissions()
            .check_permission(request)
            .await
            .map_err(|status| AuthzError(status.to_string()))?
            .into_inner();

        let allowed =
            response.permissionship == check_permission_response::Permissionship::HasPermission as i32;
        Ok(allowed)
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
                self.check_permission_inner(
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
                self.check_permission_inner(
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
