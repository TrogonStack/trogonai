use std::path::Path;

use serde_json::{Value, json};
use trogon_sandbox_contract::LeaseRequest;

pub fn registry_image(repository: &str) -> Value {
    json!({ "kind": "registry", "reference": { "registry": "registry.example", "repository": repository, "tag": "1.0" } })
}

pub fn snapshot_image() -> Value {
    json!({
        "kind": "snapshot_tag",
        "tag": { "tenant": "acme", "name": "stable" },
        "artifact_type": "sandbox_rootfs_v1",
    })
}

pub fn host_mount(path: &Path) -> Value {
    json!({ "host_mount": { "path": path.to_string_lossy(), "readonly": true } })
}

pub fn volume(bucket: &str, prefix: &str, mode: &str) -> Value {
    json!({ "s3_volume": { "bucket": bucket, "prefix": prefix, "mode": mode } })
}

pub fn clone_of(url: &str, sha: &str, auth: Value) -> Value {
    json!({ "clone": { "url": url, "sha": sha, "base_sha": null, "auth": auth } })
}

pub fn no_auth() -> Value {
    json!({ "none": {} })
}

pub struct Lease {
    pub image: Value,
    pub workspace: Value,
    pub env: Value,
    pub ttl_seconds: u32,
    pub privileged: bool,
    pub tenant: String,
    pub mounts: Value,
}

impl Lease {
    pub fn new(workspace: Value) -> Self {
        Self {
            image: registry_image("sessions/python"),
            workspace,
            env: json!({}),
            ttl_seconds: 60,
            privileged: false,
            tenant: "acme".to_owned(),
            mounts: json!([]),
        }
    }

    pub fn empty() -> Self {
        Self::new(json!({ "empty": {} }))
    }

    pub fn image(mut self, image: Value) -> Self {
        self.image = image;
        self
    }

    pub fn env(mut self, env: Value) -> Self {
        self.env = env;
        self
    }

    pub fn ttl_seconds(mut self, ttl_seconds: u32) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    pub fn with_mount(mut self) -> Self {
        self.mounts = json!([{
            "source": { "backend": "s3", "bucket": "bucket-name", "prefix": "data" },
            "mount_path": "/mnt/data",
        }]);
        self
    }

    pub fn privileged(mut self) -> Self {
        self.privileged = true;
        self
    }

    pub fn tenant(mut self, tenant: &str) -> Self {
        tenant.clone_into(&mut self.tenant);
        self
    }

    pub fn build(self) -> Result<LeaseRequest, serde_json::Error> {
        serde_json::from_value(self.json())
    }

    pub fn json(self) -> Value {
        json!({
            "tenant": self.tenant,
            "provider_pref": ["azure_container_apps"],
            "spec": {
                "image": self.image,
                "workspace": self.workspace,
                "cpu_seconds": 5,
                "memory_mib": 64,
                "env": self.env,
                "mounts": self.mounts,
                "privileged": self.privileged,
                "boot_mode": "entrypoint_default",
            },
            "ttl_seconds": self.ttl_seconds,
            "idempotency_key": "key-1",
        })
    }
}
