use crate::constants::PERSISTENT_RUNTIME_ENV;
use crate::v8_wasm_lease_error::V8WasmLeaseError;
use trogon_sandbox_contract::LeaseRequest;

/// Whether the lease opted into a persistent runtime. Absent means no; only the spellings
/// `true`, `1`, `false` and `0` are accepted, so a typo cannot silently drop the runtime.
pub(crate) fn wants_persistent_runtime(request: &LeaseRequest) -> Result<bool, V8WasmLeaseError> {
    match request.spec.env().get(PERSISTENT_RUNTIME_ENV).map(String::as_str) {
        None | Some("false" | "0") => Ok(false),
        Some("true" | "1") => Ok(true),
        Some(other) => Err(V8WasmLeaseError::InvalidPersistentRuntime(other.to_owned())),
    }
}
