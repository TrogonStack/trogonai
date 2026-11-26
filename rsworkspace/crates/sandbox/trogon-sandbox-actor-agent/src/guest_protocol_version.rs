//! Version negotiation between a worker and the in-actor agent.

use crate::GuestProtocolError;

/// Version of this HTTP contract.
///
/// It is bumped when a path, a body shape or a header changes in a way an older peer cannot ignore. Additive,
/// serde-defaulted fields do not bump it.
pub const GUEST_PROTOCOL_VERSION: u32 = 1;

/// Oldest worker the agent still serves.
///
/// A worker that predates the handshake sends no version header and counts as version 0, so it is refused by default.
/// An operator who still runs an older worker against a new guest image sets [`MIN_WORKER_PROTOCOL_VERSION_ENV`] to 0
/// on the agent until the workers are upgraded.
pub const MIN_SUPPORTED_WORKER_PROTOCOL_VERSION: u32 = 1;

/// Oldest agent the worker still drives, from the same rule applied the other way.
///
/// An operator who still boots old guest images sets [`MIN_AGENT_PROTOCOL_VERSION_ENV`] to 0 on the worker until the
/// images are rebuilt.
pub const MIN_SUPPORTED_AGENT_PROTOCOL_VERSION: u32 = 1;

/// Environment variable that overrides [`MIN_SUPPORTED_WORKER_PROTOCOL_VERSION`] on the agent.
///
/// Unset, empty or unparseable keeps the default.
pub const MIN_WORKER_PROTOCOL_VERSION_ENV: &str = "SANDBOX_MIN_WORKER_PROTOCOL_VERSION";

/// Environment variable that overrides [`MIN_SUPPORTED_AGENT_PROTOCOL_VERSION`] on the worker.
///
/// Unset, empty or unparseable keeps the default.
pub const MIN_AGENT_PROTOCOL_VERSION_ENV: &str = "SANDBOX_MIN_AGENT_PROTOCOL_VERSION";

/// Request header carrying the caller's [`GUEST_PROTOCOL_VERSION`].
pub const WORKER_PROTOCOL_VERSION_HEADER: &str = "x-sandbox-protocol-version";

/// Response header carrying the agent's [`GUEST_PROTOCOL_VERSION`].
///
/// It is present on every response to a path under the reserved prefix and on the readiness path.
pub const AGENT_PROTOCOL_VERSION_HEADER: &str = "x-sandbox-agent-protocol-version";

/// Response header carrying the oldest worker version the agent serves.
pub const AGENT_MIN_PROTOCOL_VERSION_HEADER: &str = "x-sandbox-agent-min-protocol-version";

/// HTTP status of a [`GuestProtocolError`] refusal (`Upgrade Required`).
pub const PROTOCOL_REFUSED_STATUS: u16 = 426;

/// HTTP status an agent answers for a verb it knows it does not implement.
pub const UNIMPLEMENTED_STATUS: u16 = 501;

/// Reads a minimum version override from the environment variable `name`, falling back to `default`.
pub fn min_protocol_version_from_env(name: &str, default: u32) -> u32 {
    parse_min_protocol_version(std::env::var(name).ok().as_deref(), default)
}

/// The pure half of [`min_protocol_version_from_env`].
pub fn parse_min_protocol_version(raw: Option<&str>, default: u32) -> u32 {
    raw.and_then(|value| value.trim().parse().ok()).unwrap_or(default)
}

/// Worker-side check of the agent's answer.
///
/// Reads the agent's version header (missing counts as 0) and admits it when it is at least `min_supported`. Returns
/// the agent's version.
pub fn admit_agent_version(header: Option<&str>, min_supported: u32) -> Result<u32, GuestProtocolError> {
    let agent = parse_protocol_version(header)?;
    admit_protocol_version(agent, min_supported)?;
    Ok(agent)
}

/// Reads a peer's version header. A missing header is a pre-handshake peer and counts as version 0.
pub fn parse_protocol_version(header: Option<&str>) -> Result<u32, GuestProtocolError> {
    match header {
        None => Ok(0),
        Some(raw) => raw.trim().parse().map_err(|_| GuestProtocolError::VersionMalformed),
    }
}

/// Admits `peer_version` when it is at least `min_supported`.
pub fn admit_protocol_version(peer_version: u32, min_supported: u32) -> Result<(), GuestProtocolError> {
    if peer_version >= min_supported {
        Ok(())
    } else {
        Err(GuestProtocolError::VersionTooOld {
            peer_version,
            min_supported,
            this_version: GUEST_PROTOCOL_VERSION,
        })
    }
}
