//! Version negotiation between a worker and the in-actor agent.

use trogon_std::env::ReadEnv;

use crate::GuestProtocolError;
use crate::constants::GUEST_PROTOCOL_VERSION;

/// Reads a minimum version override from the environment variable `name`, falling back to `default`.
pub fn min_protocol_version_from_env(env: &impl ReadEnv, name: &str, default: u32) -> u32 {
    parse_min_protocol_version(env.var(name).ok().as_deref(), default)
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
