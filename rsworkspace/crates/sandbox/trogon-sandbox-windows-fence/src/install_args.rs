use crate::install_options::InstallOptions;
use crate::uninstall_options::UninstallOptions;

/// The arguments of `fence-helper install` for `opts`.
pub fn install_args(opts: &InstallOptions) -> Vec<String> {
    let mut args = vec!["install".to_string()];
    if let Some(guid) = &opts.sublayer_guid {
        args.extend(["--sublayer-guid".to_string(), guid.to_string()]);
    }
    if let Some(range) = &opts.proxy_port_range {
        args.extend(["--proxy-port-range".to_string(), range.to_string()]);
    }
    if opts.allow_loopback {
        args.push("--allow-loopback".to_string());
    }
    if let Some(user) = &opts.sandbox_user {
        args.extend(["--sandbox-user".to_string(), user.to_string()]);
    }
    if opts.force {
        args.push("--force".to_string());
    }
    args
}

pub(crate) fn uninstall_args(opts: &UninstallOptions) -> Vec<String> {
    let mut args = vec!["uninstall".to_string()];
    if let Some(guid) = &opts.sublayer_guid {
        args.extend(["--sublayer-guid".to_string(), guid.to_string()]);
    }
    if opts.keep_user {
        args.push("--keep-user".to_string());
    }
    args
}

#[cfg(test)]
mod tests;
