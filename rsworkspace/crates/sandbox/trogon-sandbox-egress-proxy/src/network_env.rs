//! The environment variables that configure the proxy daemon's
//! [`NetworkOptions`], and the list a parent process forwards to a daemon it
//! starts with a cleared environment.
//!
//! Every variable is optional and an unset one keeps the compatible default,
//! so a deployment that sets none behaves as before.

use std::time::Duration;

use trogon_sandbox_contract::{
    DomainOnlyAllows, DomainPattern, IpRange, LiveFlowResweep, NetworkOptions, TlsInterceptionExemptions,
};

use crate::network_env_error::NetworkEnvError;

mod constants;
pub use constants::{
    BINDING_TTL_SECS, FORWARDED_VARS, REQUIRE_TERMINATION_FOR_NAME_ONLY, RESWEEP_LIVE_FLOWS, REVALIDATE_INTERVAL_SECS,
    TLS_EXEMPT_ADDRESSES, TLS_EXEMPT_HOSTS,
};

/// The forwarded variables `get_env` holds, as name and value pairs.
pub fn forwarded(get_env: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, String)> {
    FORWARDED_VARS
        .iter()
        .filter_map(|name| get_env(name).map(|value| (*name, value)))
        .collect()
}

/// Reads the DNS-binding and interception options from the environment.
pub fn options_from_env(get_env: impl Fn(&str) -> Option<String>) -> Result<NetworkOptions, NetworkEnvError> {
    let mut options = NetworkOptions::default();
    if let Some(ttl) = positive_secs(&get_env, BINDING_TTL_SECS)? {
        options.binding_ttl = Some(ttl);
    }
    if flag(&get_env, RESWEEP_LIVE_FLOWS)? {
        options.resweep_live_flows = LiveFlowResweep::Enabled;
    }
    if let Some(raw) = get_env(REVALIDATE_INTERVAL_SECS) {
        let secs = raw.trim().parse::<u64>().map_err(|_| NetworkEnvError::NotSeconds {
            name: REVALIDATE_INTERVAL_SECS,
        })?;
        if secs > 0 {
            options.revalidate_interval = Some(Duration::from_secs(secs));
            options.resweep_live_flows = LiveFlowResweep::Enabled;
        }
    }
    if flag(&get_env, REQUIRE_TERMINATION_FOR_NAME_ONLY)? {
        options.domain_only_allows = DomainOnlyAllows::RequireTermination;
    }
    let hostnames = list(&get_env, TLS_EXEMPT_HOSTS)
        .into_iter()
        .map(|raw| {
            raw.parse::<DomainPattern>()
                .map_err(|source| NetworkEnvError::InvalidHost {
                    name: TLS_EXEMPT_HOSTS,
                    entry: raw,
                    source,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let addresses = list(&get_env, TLS_EXEMPT_ADDRESSES)
        .into_iter()
        .map(|raw| {
            raw.parse::<IpRange>().map_err(|_| NetworkEnvError::InvalidRange {
                name: TLS_EXEMPT_ADDRESSES,
                entry: raw,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !hostnames.is_empty() || !addresses.is_empty() {
        options.tls_interception_exemptions = TlsInterceptionExemptions::new(hostnames, addresses);
    }
    Ok(options)
}

fn positive_secs(
    get_env: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<Option<Duration>, NetworkEnvError> {
    get_env(name)
        .map(|raw| {
            raw.trim()
                .parse::<u64>()
                .ok()
                .filter(|secs| *secs > 0)
                .map(Duration::from_secs)
                .ok_or(NetworkEnvError::NotPositiveSeconds { name })
        })
        .transpose()
}

fn flag(get_env: &impl Fn(&str) -> Option<String>, name: &'static str) -> Result<bool, NetworkEnvError> {
    match get_env(name).as_deref().map(str::trim) {
        None | Some("") | Some("false") | Some("0") => Ok(false),
        Some("true") | Some("1") => Ok(true),
        Some(_) => Err(NetworkEnvError::NotBoolean { name }),
    }
}

fn list(get_env: &impl Fn(&str) -> Option<String>, name: &str) -> Vec<String> {
    get_env(name)
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
