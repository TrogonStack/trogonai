use std::fmt::Display;
use std::io::Write;

use trogon_sandbox_windows_fence::{
    InstallOptions, InstallOutcome, InstallProgress, ProxyPortRange, SandboxUser, SublayerGuid, UninstallOptions,
    UninstallOutcome, WindowsFence, render_install_report,
};
use trogon_std::env::{ReadEnv, SystemEnv};

/// A command line the install or uninstall options do not accept.
#[derive(Debug, PartialEq)]
pub(crate) struct UsageError(pub(crate) String);

pub(crate) fn parse_install(args: &[String]) -> Result<InstallOptions, UsageError> {
    let mut opts = InstallOptions::default();
    let mut index = 0;
    while index < args.len() {
        let (name, inline) = split_option(&args[index]);
        index += 1;
        match name {
            "--allow-loopback" => opts.allow_loopback = true,
            "--force" => opts.force = true,
            "--skip-profile-grant" => opts.skip_profile_grant = true,
            "--sublayer-guid" => opts.sublayer_guid = Some(parse_value(name, "guid", inline, args, &mut index)?),
            "--proxy-port-range" => {
                opts.proxy_port_range = Some(parse_value::<ProxyPortRange>(name, "lo-hi", inline, args, &mut index)?)
            }
            "--sandbox-user" => {
                opts.sandbox_user = Some(parse_value::<SandboxUser>(name, "name", inline, args, &mut index)?)
            }
            "--profile-exclude" => opts
                .profile_exclude
                .push(raw_value(name, "path", inline, args, &mut index)?),
            _ => return Err(UsageError(format!("unknown option '{}'", args[index - 1]))),
        }
    }
    Ok(opts)
}

pub(crate) fn parse_uninstall(args: &[String]) -> Result<UninstallOptions, UsageError> {
    let mut opts = UninstallOptions::default();
    let mut index = 0;
    while index < args.len() {
        let (name, inline) = split_option(&args[index]);
        index += 1;
        match name {
            "--sublayer-guid" => {
                opts.sublayer_guid = Some(parse_value::<SublayerGuid>(name, "guid", inline, args, &mut index)?)
            }
            _ => return Err(UsageError(format!("unknown option '{}'", args[index - 1]))),
        }
    }
    Ok(opts)
}

fn split_option(arg: &str) -> (&str, Option<String>) {
    match arg.split_once('=') {
        Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
        _ => (arg, None),
    }
}

fn raw_value(
    name: &str,
    placeholder: &str,
    inline: Option<String>,
    args: &[String],
    index: &mut usize,
) -> Result<String, UsageError> {
    if let Some(value) = inline {
        return Ok(value);
    }
    let value = args
        .get(*index)
        .ok_or_else(|| UsageError(format!("option '{name} <{placeholder}>' argument missing")))?;
    *index += 1;
    Ok(value.clone())
}

fn parse_value<T: std::str::FromStr>(
    name: &str,
    placeholder: &str,
    inline: Option<String>,
    args: &[String],
    index: &mut usize,
) -> Result<T, UsageError>
where
    T::Err: Display,
{
    raw_value(name, placeholder, inline, args, index)?
        .parse::<T>()
        .map_err(|e| UsageError(format!("{name}: {e}")))
}

struct StderrProgress<'a, E: Write>(&'a mut E);

impl<E: Write> InstallProgress for StderrProgress<'_, E> {
    fn profile_grant_start(&mut self, notice: &str) {
        let _ = writeln!(self.0, "{notice}");
    }

    fn profile_grant_line(&mut self, line: &str) {
        let _ = writeln!(self.0, "{line}");
    }
}

pub(crate) fn run_install<O: Write, E: Write>(
    fence: &WindowsFence,
    opts: &InstallOptions,
    env: &impl ReadEnv,
    out: &mut O,
    err: &mut E,
) -> i32 {
    let outcome = fence.install(opts, &mut StderrProgress(err), env);
    match outcome {
        Ok(InstallOutcome::Cancelled) => {
            let _ = writeln!(err, "Install cancelled at the UAC prompt. Nothing changed.");
            EXIT_CANCELLED
        }
        Ok(InstallOutcome::Installed { status, profile_grant }) => {
            let _ = writeln!(out, "{}", render_install_report(&status, &profile_grant));
            0
        }
        Err(e) => {
            let _ = writeln!(err, "Error: {}", e.message());
            EXIT_FAILURE
        }
    }
}

pub(crate) fn run_uninstall<O: Write, E: Write>(
    fence: &WindowsFence,
    opts: &UninstallOptions,
    out: &mut O,
    err: &mut E,
) -> i32 {
    match fence.uninstall(opts) {
        Ok(UninstallOutcome::Cancelled) => {
            let _ = writeln!(err, "Uninstall cancelled at the UAC prompt.");
            EXIT_CANCELLED
        }
        Ok(UninstallOutcome::Removed) => {
            let _ = writeln!(out, "WFP filters and `trogon-sandbox` account removed.");
            0
        }
        Err(e) => {
            let _ = writeln!(err, "Error: {}", e.message());
            EXIT_FAILURE
        }
    }
}

#[cfg(windows)]
fn this_fence() -> Result<WindowsFence, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate trogon-sandbox-policy: {e}"))?;
    trogon_sandbox_windows_fence::FenceHelper::new(exe)
        .map(WindowsFence::new)
        .map_err(|e| e.message().to_string())
}

#[cfg(not(windows))]
fn this_fence() -> Result<WindowsFence, String> {
    Err("the Windows sandbox can only be installed on Windows".to_string())
}

/// Entry point of `trogon-sandbox-policy windows-install` and
/// `trogon-sandbox-policy windows-uninstall`; `uninstall` picks which. Returns
/// the process exit code.
pub(crate) fn run(uninstall: bool, args: &[String]) -> i32 {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    if uninstall {
        let opts = match parse_uninstall(args) {
            Ok(opts) => opts,
            Err(usage) => return usage_failure(&mut err, &usage),
        };
        match this_fence() {
            Ok(fence) => run_uninstall(&fence, &opts, &mut out, &mut err),
            Err(message) => failure(&mut err, &message),
        }
    } else {
        let opts = match parse_install(args) {
            Ok(opts) => opts,
            Err(usage) => return usage_failure(&mut err, &usage),
        };
        match this_fence() {
            Ok(fence) => run_install(&fence, &opts, &SystemEnv, &mut out, &mut err),
            Err(message) => failure(&mut err, &message),
        }
    }
}

fn usage_failure(err: &mut impl Write, usage: &UsageError) -> i32 {
    let _ = writeln!(err, "error: {}", usage.0);
    EXIT_USAGE
}

fn failure(err: &mut impl Write, message: &str) -> i32 {
    let _ = writeln!(err, "Error: {message}");
    EXIT_FAILURE
}

mod constants;
#[cfg(all(test, unix))]
mod tests;
use constants::{EXIT_CANCELLED, EXIT_FAILURE, EXIT_USAGE};
