//! SpiceDB-backed authorization checks from CEL policy.
//!
//! Fully consistent reads are required so policy decisions observe the latest tuple
//! state; snapshot reads would allow stale allows or denials during replication lag.

use std::sync::Arc;

use cel_interpreter::Value;
use crate::authz::{AuthzContext, AuthzError, PermissionChecker};

use super::errors::{CelBuiltinsError, HostFailure};
use super::value::expect_string;

pub(crate) const BUILTIN_NAME: &str = "spicedb.check";

pub trait SpicedbHostBackend: Send + Sync {
    fn check(&self, subject: &str, permission: &str, resource: &str) -> Result<bool, HostFailure>;
}

/// Bridges [`PermissionChecker`] into the synchronous CEL host surface.
pub struct PermissionCheckerSpicedbBackend {
    checker: Arc<dyn PermissionChecker>,
    tenant: Option<String>,
    caller_sub: Option<String>,
    session_id: Option<String>,
    server_id: String,
}

impl PermissionCheckerSpicedbBackend {
    #[must_use]
    pub fn new(
        checker: Arc<dyn PermissionChecker>,
        tenant: Option<String>,
        caller_sub: Option<String>,
        session_id: Option<String>,
        server_id: String,
    ) -> Self {
        Self {
            checker,
            tenant,
            caller_sub,
            session_id,
            server_id,
        }
    }
}

fn parse_tool_resource(resource: &str) -> Result<(&str, &str), HostFailure> {
    let rest = resource
        .strip_prefix("tool:")
        .ok_or(HostFailure::Permanent)?;
    let (server_id, tool_name) = rest.split_once('|').ok_or(HostFailure::Permanent)?;
    if server_id.is_empty() || tool_name.is_empty() {
        return Err(HostFailure::Permanent);
    }
    Ok((server_id, tool_name))
}

impl SpicedbHostBackend for PermissionCheckerSpicedbBackend {
    fn check(&self, _subject: &str, permission: &str, resource: &str) -> Result<bool, HostFailure> {
        if permission == "invoke" {
            let (server_id, tool_name) = parse_tool_resource(resource)?;
            let server_id = if server_id.is_empty() {
                self.server_id.as_str()
            } else {
                server_id
            };
            let result = futures::executor::block_on(self.checker.authorize_mcp_request(
                AuthzContext {
                    tenant: self.tenant.as_deref(),
                    caller_sub: self.caller_sub.as_deref(),
                    identity_source: crate::authz::IdentitySource::Jwt,
                    server_id,
                    session_id: self.session_id.as_deref(),
                    jsonrpc_method: "tools/call",
                    tool_name: Some(tool_name),
                    resource_uri: None,
                },
            ));
            return map_authz_result(result);
        }

        if permission == "read" {
            let uri = resource
                .strip_prefix("resource:")
                .unwrap_or(resource);
            if uri.is_empty() {
                return Err(HostFailure::Permanent);
            }
            let result = futures::executor::block_on(self.checker.authorize_mcp_request(
                AuthzContext {
                    tenant: self.tenant.as_deref(),
                    caller_sub: self.caller_sub.as_deref(),
                    identity_source: crate::authz::IdentitySource::Jwt,
                    server_id: self.server_id.as_str(),
                    session_id: self.session_id.as_deref(),
                    jsonrpc_method: "resources/read",
                    tool_name: None,
                    resource_uri: Some(uri),
                },
            ));
            return map_authz_result(result);
        }

        Err(HostFailure::Permanent)
    }
}

fn map_authz_result(result: Result<bool, AuthzError>) -> Result<bool, HostFailure> {
    match result {
        Ok(allowed) => Ok(allowed),
        Err(_) => Err(HostFailure::Transient),
    }
}

pub fn check(subject: Value, permission: Value, resource: Value) -> Result<Value, CelBuiltinsError> {
    use super::context::current_host_eval;

    let host = current_host_eval().ok_or(CelBuiltinsError::authz_unreachable(
        BUILTIN_NAME,
        "host eval context missing",
    ))?;
    let subject = expect_string(subject, BUILTIN_NAME, 0)?;
    let permission = expect_string(permission, BUILTIN_NAME, 1)?;
    let resource = expect_string(resource, BUILTIN_NAME, 2)?;
    let allowed = host.spicedb_check(subject.as_str(), permission.as_str(), resource.as_str())?;
    Ok(Value::Bool(allowed))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use cel_interpreter::Value;

    use super::{BUILTIN_NAME, PermissionCheckerSpicedbBackend, SpicedbHostBackend, check};
    use crate::authz::{AllowAllPermissionChecker, AuthzContext, AuthzError, PermissionChecker};
    use crate::cel_builtins::context::{with_host_eval, HostEvalContext};
    use crate::cel_builtins::errors::HostFailure;

    fn s(v: &str) -> Value {
        Value::from(v.to_string())
    }

    struct FailChecker;

    #[async_trait::async_trait]
    impl PermissionChecker for FailChecker {
        async fn authorize_mcp_request(&self, _ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
            Err(AuthzError("pdp down".into()))
        }
    }

    struct CountingChecker(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl PermissionChecker for CountingChecker {
        async fn authorize_mcp_request(&self, _ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }
    }

    struct MockBackend {
        outcome: Result<bool, HostFailure>,
    }

    impl SpicedbHostBackend for MockBackend {
        fn check(&self, _subject: &str, _permission: &str, _resource: &str) -> Result<bool, HostFailure> {
            self.outcome
        }
    }

    fn host_with_backend(backend: Arc<dyn SpicedbHostBackend>) -> HostEvalContext {
        HostEvalContext::for_tests().with_spicedb(backend)
    }

    #[test]
    fn check_allows_when_backend_allows() {
        let host = host_with_backend(Arc::new(MockBackend {
            outcome: Ok(true),
        }));
        let result = with_host_eval(&host, || {
            check(
                s("user:alice"),
                s("invoke"),
                s("tool:github|create_issue"),
            )
        })
        .unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn check_returns_false_on_deny_not_error() {
        let host = host_with_backend(Arc::new(MockBackend {
            outcome: Ok(false),
        }));
        let result = with_host_eval(&host, || {
            check(
                s("user:alice"),
                s("invoke"),
                s("tool:github|create_issue"),
            )
        })
        .unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn check_transient_failure_classifies_authz_unreachable() {
        let host = host_with_backend(Arc::new(MockBackend {
            outcome: Err(HostFailure::Transient),
        }));
        let err = with_host_eval(&host, || {
            check(
                s("user:alice"),
                s("invoke"),
                s("tool:github|create_issue"),
            )
        })
        .unwrap_err();
        assert_eq!(err.host_failure(), Some(HostFailure::Transient));
        assert!(err.to_string().contains("authz_unreachable"));
    }

    #[test]
    fn permission_checker_backend_invokes_checker() {
        let counter = Arc::new(AtomicUsize::new(0));
        let backend = PermissionCheckerSpicedbBackend::new(
            Arc::new(CountingChecker(counter.clone())),
            Some("acme".into()),
            Some("alice".into()),
            Some("sess-1".into()),
            "github".into(),
        );
        let allowed = backend
            .check("user:alice", "invoke", "tool:github|create_issue")
            .unwrap();
        assert!(allowed);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn permission_checker_backend_maps_transport_error_to_transient() {
        let backend = PermissionCheckerSpicedbBackend::new(
            Arc::new(FailChecker),
            None,
            None,
            None,
            "github".into(),
        );
        assert_eq!(
            backend.check("user:alice", "invoke", "tool:github|create_issue"),
            Err(HostFailure::Transient)
        );
    }

    #[test]
    fn permission_checker_backend_malformed_resource_is_permanent() {
        let backend = PermissionCheckerSpicedbBackend::new(
            Arc::new(AllowAllPermissionChecker),
            None,
            None,
            None,
            "github".into(),
        );
        assert_eq!(
            backend.check("user:alice", "invoke", "not-a-tool-ref"),
            Err(HostFailure::Permanent)
        );
    }

    #[test]
    fn check_requires_host_context() {
        let err = check(Value::Null, Value::Null, Value::Null).unwrap_err();
        assert_eq!(err.host_failure(), Some(HostFailure::Transient));
        assert!(err.to_string().contains(BUILTIN_NAME));
    }
}
