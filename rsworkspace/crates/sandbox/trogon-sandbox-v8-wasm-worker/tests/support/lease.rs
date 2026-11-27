use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseRequest, LifecycleEvent, RunCommandRequest};
use trogon_sandbox_protocol::LifecycleEmitter;
use trogon_sandbox_v8_wasm_worker::{PERSISTENT_RUNTIME_ENV, V8WasmProvider};

/// Records every lifecycle event a provider emits.
#[derive(Default)]
pub struct CapturingEmitter {
    events: Mutex<Vec<LifecycleEvent>>,
}

impl CapturingEmitter {
    pub fn events(&self) -> Vec<LifecycleEvent> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn printed(&self, needle: &str) -> bool {
        self.events()
            .iter()
            .any(|event| matches!(event, LifecycleEvent::Output { text, .. } if text.contains(needle)))
    }

    pub fn completed(&self) -> bool {
        self.events()
            .iter()
            .any(|event| matches!(event, LifecycleEvent::Completed { .. }))
    }
}

#[async_trait]
impl LifecycleEmitter for CapturingEmitter {
    async fn emit(&self, event: LifecycleEvent) {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

pub fn provider() -> (Arc<CapturingEmitter>, Arc<V8WasmProvider>) {
    let emitter = Arc::new(CapturingEmitter::default());
    let provider = Arc::new(V8WasmProvider::with_emitter(emitter.clone()));
    (emitter, provider)
}

pub fn registry_image(repository: &str) -> Value {
    json!({ "kind": "registry", "reference": { "registry": "registry.example", "repository": repository } })
}

pub fn artifact_reference(repository: &str) -> Value {
    json!({
        "registry": "registry.example",
        "repository": repository,
        "digest": format!("sha256:{}", "00".repeat(32)),
        "artifact_type": "sandbox_rootfs_v1",
    })
}

pub fn host_mount(path: &Path) -> Value {
    json!({ "host_mount": { "path": path.to_string_lossy(), "readonly": true } })
}

pub struct Lease {
    pub image: Value,
    pub workspace: Value,
    pub env: Value,
    pub cpu_seconds: u32,
    pub privileged: bool,
}

impl Lease {
    pub fn new(workspace: Value, guest_kind: &str) -> Self {
        Self {
            image: registry_image("placeholder"),
            workspace,
            env: json!({ "V8_WASM_GUEST": guest_kind }),
            cpu_seconds: 5,
            privileged: false,
        }
    }

    pub fn mounted(dir: &Path, guest_kind: &str) -> Self {
        Self::new(host_mount(dir), guest_kind)
    }

    pub fn persistent(mut self) -> Self {
        self.env[PERSISTENT_RUNTIME_ENV] = json!("true");
        self
    }

    pub fn image(mut self, image: Value) -> Self {
        self.image = image;
        self
    }

    pub fn env(mut self, env: Value) -> Self {
        self.env = env;
        self
    }

    pub fn cpu_seconds(mut self, seconds: u32) -> Self {
        self.cpu_seconds = seconds;
        self
    }

    pub fn privileged(mut self) -> Self {
        self.privileged = true;
        self
    }

    pub fn build(self) -> Result<LeaseRequest, serde_json::Error> {
        serde_json::from_value(self.json())
    }

    pub fn json(self) -> Value {
        json!({
            "tenant": "acme",
            "provider_pref": ["v8_wasm"],
            "spec": {
                "image": self.image,
                "workspace": self.workspace,
                "cpu_seconds": self.cpu_seconds,
                "memory_mib": 64,
                "env": self.env,
                "privileged": self.privileged,
                "boot_mode": "entrypoint_default",
            },
            "ttl_seconds": 60,
            "idempotency_key": "key-1",
        })
    }
}

pub fn eval_request(lease: &str, source: &str) -> Result<RunCommandRequest, serde_json::Error> {
    command_request(lease, json!({ "command": ["eval", source] }))
}

pub fn command_request(lease: &str, mut fields: Value) -> Result<RunCommandRequest, serde_json::Error> {
    fields["lease_id"] = json!(lease);
    fields["timeout_seconds"] = fields.get("timeout_seconds").cloned().unwrap_or(json!(5));
    serde_json::from_value(fields)
}

/// The lease error a failed `lease` call carried, so tests match variants instead of messages.
pub fn lease_error(
    error: Option<trogon_sandbox_protocol::ProviderError>,
    expectation: &str,
) -> Result<trogon_sandbox_v8_wasm_worker::V8WasmLeaseError, Box<dyn std::error::Error>> {
    match error.ok_or(expectation)? {
        trogon_sandbox_protocol::ProviderError::Backend(source) => Ok(*source
            .downcast::<trogon_sandbox_v8_wasm_worker::V8WasmLeaseError>()
            .map_err(|_| "the backend error is not a lease error")?),
        other => Err(other.into()),
    }
}
