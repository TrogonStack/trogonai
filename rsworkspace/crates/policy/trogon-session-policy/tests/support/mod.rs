#![allow(clippy::expect_used)]

use std::io;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use trogon_session_policy::*;

pub fn document() -> Value {
    json!({
        "version": 1,
        "privacy": {"mode": "local_only", "allowed_destinations": ["local"]},
        "destinations": [
            {"id": "local", "location": "local", "host": "192.0.2.10", "port": 8888},
            {"id": "cloud", "location": "subscription", "host": "api.example.com", "port": 443}
        ],
        "model_routes": [
            {"purpose": "worker", "destination": "local", "model": "test-worker"},
            {"purpose": "reviewer", "destination": "local", "model": "test-reviewer"}
        ],
        "execution": {
            "network_allowlist": [],
            "limits": {"command_seconds": 30, "memory_mib": 512, "processes": 64, "payload_bytes": 4096, "model_calls": 16, "tool_calls": 8},
            "wasm_tools": [
                {"id": "read-note", "component": format!("sha256:{}", "a".repeat(64)), "capabilities": {"preopens": [{"host_dir": "/task/input", "guest_path": "/input", "readonly": true}]}, "review": "deterministic_only"},
                {"id": "pure-tool", "component": format!("sha256:{}", "b".repeat(64)), "capabilities": {}, "review": "required_veto"}
            ],
            "shell": {"isolation": "micro_vm", "review": "required_veto"}
        }
    })
}

pub fn policy(document: &Value) -> SessionPolicy {
    SessionPolicy::parse(&document.to_string()).expect("valid fixture policy")
}

#[derive(Debug, Default)]
pub struct Observed {
    pub models: Vec<(ModelPurpose, String, String)>,
    pub wasm: Vec<WasmTool>,
    pub shell: Vec<ShellGrant>,
    pub reviews: Vec<(String, Operation)>,
    pub denials: Vec<Denial>,
    pub fail_audit: bool,
    pub fail_review: bool,
    pub fail_effect: bool,
    pub veto: bool,
}

#[derive(Clone, Default)]
pub struct Backend(pub Arc<Mutex<Observed>>);

#[async_trait::async_trait]
impl ModelExecutor for Backend {
    type Output = (String, String);
    type Error = io::Error;
    async fn model(&mut self, purpose: ModelPurpose, route: &ModelRoute, _: &str) -> Result<Self::Output, Self::Error> {
        let mut observed = self.0.lock().expect("test mutex");
        observed
            .models
            .push((purpose, route.destination().host().into(), route.model().into()));
        if observed.fail_effect {
            return Err(io::Error::other("synthetic effect failure"));
        }
        Ok((route.destination().host().into(), route.model().into()))
    }
}

#[async_trait::async_trait]
impl WasmExecutor for Backend {
    type Output = ();
    type Error = io::Error;
    async fn wasm(&mut self, tool: &WasmTool, _: &str) -> Result<(), Self::Error> {
        self.0.lock().expect("test mutex").wasm.push(tool.clone());
        Ok(())
    }
}

#[async_trait::async_trait]
impl ShellExecutor for Backend {
    type Output = ();
    type Error = io::Error;
    async fn shell(&mut self, grant: &ShellGrant, _: &str) -> Result<(), Self::Error> {
        self.0.lock().expect("test mutex").shell.push(grant.clone());
        Ok(())
    }
}

#[async_trait::async_trait]
impl ReviewExecutor for Backend {
    type Error = io::Error;
    async fn review(&mut self, route: &ModelRoute, operation: Operation, _: &str) -> Result<ReviewVeto, Self::Error> {
        let mut observed = self.0.lock().expect("test mutex");
        observed.reviews.push((route.model().into(), operation));
        if observed.fail_review {
            return Err(io::Error::other("synthetic review failure"));
        }
        Ok(if observed.veto {
            ReviewVeto::Veto
        } else {
            ReviewVeto::NoVeto
        })
    }
}

#[async_trait::async_trait]
impl AuditSink for Backend {
    type Error = io::Error;
    async fn record_denial(&mut self, denial: &Denial) -> Result<(), Self::Error> {
        let mut observed = self.0.lock().expect("test mutex");
        if observed.fail_audit {
            return Err(io::Error::other("synthetic audit failure"));
        }
        observed.denials.push(denial.clone());
        Ok(())
    }
}

pub struct Host(SessionHost<Backend, Backend, Backend, Backend, Backend>);

impl std::ops::Deref for Host {
    type Target = SessionHost<Backend, Backend, Backend, Backend, Backend>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for Host {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Host {
    pub fn model(
        &mut self,
        purpose: ModelPurpose,
        content: &str,
    ) -> Result<(String, String), OperationError<io::Error, io::Error>> {
        futures::executor::block_on(self.0.model(purpose, content))
    }
    pub fn shell(&mut self, command: &str) -> Result<(), ShellError<io::Error, io::Error, io::Error>> {
        futures::executor::block_on(self.0.shell(command))
    }
    pub fn wasm(&mut self, tool: &ToolId, content: &str) -> Result<(), ShellError<io::Error, io::Error, io::Error>> {
        futures::executor::block_on(self.0.wasm(tool, content))
    }
    pub fn require_same_policy(
        &mut self,
        fingerprint: PolicyFingerprint,
    ) -> Result<(), OperationError<std::convert::Infallible, io::Error>> {
        futures::executor::block_on(self.0.require_same_policy(fingerprint))
    }
}

pub fn host(document: &Value) -> (Host, Backend) {
    let backend = Backend::default();
    let host = SessionHost::new(
        SessionId::new("session-1").expect("valid id"),
        policy(document),
        backend.clone(),
        backend.clone(),
        backend.clone(),
        backend.clone(),
        backend.clone(),
    );
    (Host(host), backend)
}
