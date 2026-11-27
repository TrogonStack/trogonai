use crate::constants::PERSISTENT_RUNTIME_ENV;
use trogon_sandbox_contract::LeaseRequest;

/// Whether the lease opted into a persistent runtime.
pub(crate) fn wants_persistent_runtime(request: &LeaseRequest) -> bool {
    matches!(
        request.spec.env().get(PERSISTENT_RUNTIME_ENV).map(String::as_str),
        Some("true" | "1")
    )
}
