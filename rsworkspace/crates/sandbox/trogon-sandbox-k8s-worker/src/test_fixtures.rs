use serde_json::{Value, json};
use trogon_sandbox_contract::LeaseRequest;

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A minimal lease request for the Kubernetes provider, as the broker would send it.
pub(crate) fn lease_json(privileged: bool) -> Value {
    json!({
        "tenant": "acme",
        "provider_pref": ["k8s_micro_vm"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": { "registry": "registry.example", "repository": "sessions/python", "tag": "1.0" },
            },
            "workspace": { "empty": {} },
            "cpu_seconds": 5,
            "memory_mib": 512,
            "env": {},
            "mounts": [],
            "privileged": privileged,
            "boot_mode": "entrypoint_default",
        },
        "ttl_seconds": 60,
        "idempotency_key": "key-1",
    })
}

pub(crate) fn lease_request(privileged: bool) -> Result<LeaseRequest, serde_json::Error> {
    serde_json::from_value(lease_json(privileged))
}

/// A stdin write of `bytes`, closing stdin when `eof` is set.
#[cfg(test)]
pub(crate) fn stdin_write(
    bytes: &[u8],
    eof: bool,
) -> Result<trogon_sandbox_protocol::StdinWrite, Box<dyn std::error::Error>> {
    Ok(trogon_sandbox_protocol::StdinWrite {
        data: trogon_sandbox_contract::StdinChunk::new(bytes.to_vec())?,
        eof: if eof {
            trogon_sandbox_contract::StdinEof::Close
        } else {
            trogon_sandbox_contract::StdinEof::Keep
        },
    })
}
