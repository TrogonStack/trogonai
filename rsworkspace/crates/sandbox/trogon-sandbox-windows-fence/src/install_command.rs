use crate::constants::DEFAULT_PROXY_PORT_RANGE;
use crate::constants::DEFAULT_SANDBOX_USER;
use crate::install_context::InstallContext;

/// Install flags for `ctx`. `force` only when the install replaces a set that
/// is in place with another configuration: it refuses that without `--force`
/// (exit 13), and needs none otherwise.
fn install_flags(ctx: &InstallContext, force: bool) -> String {
    let mut flags = String::new();
    if let Some(guid) = &ctx.sublayer_guid {
        flags.push_str(&format!(" --sublayer-guid {guid}"));
    }
    let range = ctx.proxy_port_range.unwrap_or(DEFAULT_PROXY_PORT_RANGE);
    if range != DEFAULT_PROXY_PORT_RANGE {
        flags.push_str(&format!(" --proxy-port-range {range}"));
    }
    if let Some(user) = &ctx.sandbox_user
        && user.as_str() != DEFAULT_SANDBOX_USER
    {
        flags.push_str(&format!(" --sandbox-user {user}"));
    }
    if ctx.allow_loopback {
        flags.push_str(" --allow-loopback");
    }
    if force {
        flags.push_str(" --force");
    }
    flags
}

/// The command line that performs the install `ctx` describes.
pub fn install_command(ctx: &InstallContext, force: bool) -> String {
    format!("trogon-sandbox windows-install{}", install_flags(ctx, force))
}
