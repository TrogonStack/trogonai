//! In-process decision traces keyed by JSON-RPC request id (Phase-1 stand-in for `agctl trace`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::authz::IdentitySource;

#[derive(Clone, Debug, Serialize)]
pub struct DecisionTrace {
    pub subject_in: String,
    pub subject_out: String,
    pub jsonrpc_method: String,
    pub cel_requires_spicedb: bool,
    pub spicedb_allowed: Option<bool>,
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_sub: Option<String>,
    pub identity_source: IdentitySource,
}

#[derive(Clone, Default)]
pub struct TraceStore {
    inner: Arc<Mutex<HashMap<String, DecisionTrace>>>,
}

impl TraceStore {
    pub fn insert(&self, request_id: impl Into<String>, trace: DecisionTrace) {
        let mut guard = self.inner.lock().expect("trace mutex poisoned");
        if guard.len() > 10_000 {
            guard.clear();
        }
        guard.insert(request_id.into(), trace);
    }

    pub fn get(&self, request_id: &str) -> Option<DecisionTrace> {
        self.inner.lock().expect("trace mutex poisoned").get(request_id).cloned()
    }
}
