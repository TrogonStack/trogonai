use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::subjects::provider_token;

/// Escapes every byte outside `[A-Za-z0-9-]` as `=HH`, so distinct worker ids
/// never share a KV key and the key stays valid for NATS.
fn escape_worker_id(worker_id: &str) -> String {
    let mut out = String::with_capacity(worker_id.len());
    for byte in worker_id.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' {
            out.push(byte as char);
        } else {
            out.push_str(&format!("={byte:02X}"));
        }
    }
    out
}

pub fn fence_kv_key(provider: ProviderKind, worker_id: &WorkerId) -> String {
    format!("{}.{}", provider_token(provider), escape_worker_id(worker_id.as_str()))
}
