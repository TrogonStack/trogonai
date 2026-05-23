//! Phase 1 — SpiceDB client, BulkCheckPermission, tuples, ZedToken cache.
//!
//! Expanded beyond [`super::spicedb_gateway_client`] for **A2A_TODO** Phase 1:
//! - **`BulkCheckPermission`** for catalog shaping on `discover`
//! - Per-method resource tuples (plan §SpiceDB table)
//! - Owner tuples on task lifecycle (`message/send` / `message/stream` create; terminal removes)
//! - ZedToken cache per session

use std::error::Error;
use std::future::{ready, Future, Ready};

/// Wire-shaped relationship tuple placeholder until real Authzed types land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TupleWireStub {
    pub relation: String,
    pub resource: String,
    pub subject: String,
}

/// Session-scoped ZedToken consistency carrier passed into bulk checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsistencyHint {
    pub zed_token: String,
}

impl ConsistencyHint {
    pub fn minimize_latency() -> Self {
        Self {
            zed_token: String::new(),
        }
    }
}

/// One permission decision from a bulk check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulkCheckOutcome {
    pub resource_id: String,
    pub allow: bool,
    pub zed_token: String,
}

/// One A2A gateway method's SpiceDB check specification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodPermissionSpec {
    pub method: String,
    pub permission: String,
    pub resource_pattern: String,
}

/// Compile-only async seam helper.
pub type GatewayReadyUnit = Ready<()>;

/// Gateway ingress coordinating catalog shaping and tuple writes.
pub trait SpiceDbGatewayIngress: Send + Sync {
    fn bulk_catalog_shape(
        &self,
        session_key: &str,
        agent_ids: &[String],
    ) -> impl Future<Output = ()> + Send;
}

/// **`BulkCheckPermission`** for catalog shaping — filters discovery AgentCards by `view`.
pub trait BulkCatalogGate: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    fn bulk_check_view(
        &self,
        consistency: ConsistencyHint,
        agent_ids: &[String],
    ) -> impl Future<Output = Result<Vec<BulkCheckOutcome>, Self::Error>> + Send;
}

/// Derives SpiceDB resource tuples per A2A method (subject / permission / resource).
pub trait MethodTupleBuilder: Send + Sync {
    fn permission_spec(&self, method: &str) -> Option<MethodPermissionSpec>;

    fn build_check_tuple(
        &self,
        method: &str,
        subject: &str,
        resource_id: &str,
    ) -> Option<TupleWireStub>;
}

/// Owner relationship lifecycle for tasks created by `message/send` or `message/stream`.
pub trait TaskOwnerTuples: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    fn write_owner(
        &self,
        task_id: &str,
        owner_sub: &str,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn remove_owner(
        &self,
        task_id: &str,
        owner_sub: &str,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn owner_tuple(task_id: &str, owner_sub: &str) -> TupleWireStub {
        TupleWireStub {
            relation: "owner".to_owned(),
            resource: format!("task:{task_id}"),
            subject: format!("user:{owner_sub}"),
        }
    }
}

/// Per-session ZedToken cache for consistent bulk checks.
pub mod session_cache {
    use super::ConsistencyHint;

    pub trait ZedTokCacheTrait: Send + Sync {
        fn cached_hint(&self, session_key: &str) -> Option<ConsistencyHint>;

        fn record_hint(&self, session_key: &str, hint: ConsistencyHint);

        fn invalidate(&self, session_key: &str);
    }
}

/// Static plan §SpiceDB method → permission mapping (compile-only reference table).
#[derive(Debug, Default)]
pub struct A2aMethodTupleTable;

impl MethodTupleBuilder for A2aMethodTupleTable {
    fn permission_spec(&self, method: &str) -> Option<MethodPermissionSpec> {
        let (permission, resource_pattern) = match method {
            "agent/getAuthenticatedExtendedCard" => ("view", "agent:{agent_id}"),
            "discover" => ("view", "agent:{agent_id}"),
            "message/send" => ("invoke", "agent:{agent_id}"),
            "message/stream" => ("invoke_stream", "agent:{agent_id}"),
            "tasks/get" | "tasks/resubscribe" => ("read", "task:{task_id}"),
            "tasks/cancel" => ("cancel", "task:{task_id}"),
            m if m.starts_with("tasks/pushNotificationConfig/") => {
                ("configure_push", "task:{task_id}")
            }
            _ => return None,
        };
        Some(MethodPermissionSpec {
            method: method.to_owned(),
            permission: permission.to_owned(),
            resource_pattern: resource_pattern.to_owned(),
        })
    }

    fn build_check_tuple(
        &self,
        method: &str,
        subject: &str,
        resource_id: &str,
    ) -> Option<TupleWireStub> {
        let spec = self.permission_spec(method)?;
        let resource = spec
            .resource_pattern
            .replace("{agent_id}", resource_id)
            .replace("{task_id}", resource_id);
        Some(TupleWireStub {
            relation: spec.permission,
            resource,
            subject: format!("user:{subject}"),
        })
    }
}

/// No-op ingress stub for compile-only wiring exercises.
#[derive(Debug, Default)]
pub struct NoopSpiceDbGateway;

impl SpiceDbGatewayIngress for NoopSpiceDbGateway {
    fn bulk_catalog_shape(
        &self,
        _session_key: &str,
        _agent_ids: &[String],
    ) -> impl Future<Output = ()> + Send {
        ready(())
    }
}

/// In-memory session ZedToken cache stub.
#[derive(Debug, Default)]
pub struct InMemoryZedTokCache {
    hints: std::sync::Mutex<std::collections::HashMap<String, ConsistencyHint>>,
}

impl session_cache::ZedTokCacheTrait for InMemoryZedTokCache {
    fn cached_hint(&self, session_key: &str) -> Option<ConsistencyHint> {
        self.hints
            .lock()
            .ok()
            .and_then(|g| g.get(session_key).cloned())
    }

    fn record_hint(&self, session_key: &str, hint: ConsistencyHint) {
        if let Ok(mut g) = self.hints.lock() {
            g.insert(session_key.to_owned(), hint);
        }
    }

    fn invalidate(&self, session_key: &str) {
        if let Ok(mut g) = self.hints.lock() {
            g.remove(session_key);
        }
    }
}
