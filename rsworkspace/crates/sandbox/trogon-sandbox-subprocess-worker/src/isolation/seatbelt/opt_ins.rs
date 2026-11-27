//! What a lease can open up on top of the baseline, each of it off by
//! default.

use trogon_sandbox_contract::MacosOptions;
use trogon_sandbox_protocol::{MachServiceMatch, mach_service_matcher};

use super::sbpl::{AllowRule, Filter, Operation, SbplError};

pub fn opt_in_allows(options: &MacosOptions) -> Result<Vec<AllowRule>, SbplError> {
    let mut rules = Vec::new();
    if options.allow_trust_service.is_allowed() {
        rules.push(AllowRule::new(
            [Operation::MachLookup],
            [Filter::global_name(TRUST_SERVICE)?],
        ));
    }
    for service in &options.allow_mach_lookup {
        let filter = match mach_service_matcher(service) {
            MachServiceMatch::Exact(name) => Filter::global_name(name)?,
            MachServiceMatch::Prefix(prefix) => Filter::global_name_prefix(prefix)?,
        };
        rules.push(AllowRule::new([Operation::MachLookup], [filter]));
    }
    if options.allow_apple_events.is_allowed() {
        rules.push(AllowRule::new([Operation::AppleeventSend], []));
        rules.push(AllowRule::new([Operation::Lsopen], []));
        for service in APPLE_EVENTS_SERVICES {
            rules.push(AllowRule::new([Operation::MachLookup], [Filter::global_name(service)?]));
        }
    }
    if options.allow_all_unix_sockets.is_allowed() {
        rules.push(unix_socket_domain());
        rules.push(AllowRule::new(
            [Operation::NetworkBind],
            [Filter::local_unix_socket(Filter::path_regex("^/")?)],
        ));
        rules.push(AllowRule::new(
            [Operation::NetworkOutbound],
            [Filter::remote_unix_socket(Filter::path_regex("^/")?)],
        ));
    } else if !options.allow_unix_sockets.is_empty() {
        rules.push(unix_socket_domain());
        for path in &options.allow_unix_sockets {
            rules.push(AllowRule::new(
                [Operation::NetworkBind],
                [Filter::local_unix_socket(Filter::subpath(path.as_str())?)],
            ));
            rules.push(AllowRule::new(
                [Operation::NetworkOutbound],
                [Filter::remote_unix_socket(Filter::subpath(path.as_str())?)],
            ));
        }
    }
    if options.allow_local_binding.is_allowed() {
        rules.push(AllowRule::new([Operation::NetworkBind], [Filter::local_ip("*:*")?]));
        rules.push(AllowRule::new([Operation::NetworkInbound], [Filter::local_ip("*:*")?]));
        rules.push(AllowRule::new(
            [Operation::NetworkOutbound],
            [Filter::remote_ip("localhost:*")?],
        ));
    }
    if options.allow_pty.is_allowed() {
        let devices =
            || -> Result<Vec<Filter>, SbplError> { Ok(vec![Filter::literal(PTY_DEVICE)?, Filter::regex(PTY_SLAVES)?]) };
        rules.push(AllowRule::new([Operation::PseudoTty], []));
        rules.push(AllowRule::new([Operation::FileIoctl], devices()?));
        rules.push(AllowRule::new(
            [Operation::FileReadStar, Operation::FileWriteStar],
            devices()?,
        ));
    }
    Ok(rules)
}

fn unix_socket_domain() -> AllowRule {
    AllowRule::new([Operation::SystemSocket], [Filter::SocketDomainUnix])
}

#[cfg(test)]
mod tests;
use super::constants::{APPLE_EVENTS_SERVICES, PTY_DEVICE, PTY_SLAVES, TRUST_SERVICE};
