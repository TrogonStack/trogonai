use trogon_sandbox_contract::SandboxEntityRecord;

/// A record together with the KV revision it was read at, for compare-and-swap.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Versioned {
    pub record: SandboxEntityRecord,
    pub revision: u64,
}
