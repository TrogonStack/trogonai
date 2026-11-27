use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use trogon_sandbox_contract::{TerminalId, TerminalIdError};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A terminal id for a client that did not pick one.
pub(crate) fn fresh_terminal_id() -> Result<TerminalId, TerminalIdError> {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    hasher.write_u128(nanos);
    TerminalId::new(format!("mcp-{:016x}", hasher.finish()))
}

#[cfg(test)]
mod tests;
