//! The command line front end of `trogon-sandbox-policy-exec`: runs one
//! command under a settings file, or serves the policy proxy on its own.

#[cfg(any(unix, windows))]
mod args;
#[cfg(any(unix, windows))]
mod control;
#[cfg(any(unix, windows))]
mod debug;
#[cfg(any(unix, windows))]
mod invocation;
#[cfg(any(unix, windows))]
mod option_scan;
#[cfg(unix)]
mod proxy_cmd;
#[cfg(unix)]
mod run;
#[cfg(any(unix, windows))]
mod shell;
#[cfg(any(unix, windows))]
mod source;
#[cfg_attr(unix, allow(dead_code))]
mod windows_cmd;
#[cfg(windows)]
mod windows_run_cmd;

#[cfg(any(unix, windows))]
use std::io::Write;

#[cfg(any(unix, windows))]
use invocation::Invocation;

/// Runs the `trogon-sandbox-policy` command line on the words after the
/// program name and returns the exit status.
#[cfg(unix)]
pub fn run_cli(operands: &[String], version: &str) -> i32 {
    let args = match Invocation::parse(operands, version) {
        Invocation::Run(args) => args,
        Invocation::Proxy(words) => return proxy_cmd::run(&words),
        Invocation::Print(text) => return print_out(&text),
        Invocation::Fail(text) => return print_err(&text),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return print_err(&format!("Error: tokio runtime: {error}\n")),
    };
    runtime.block_on(run::run(args))
}

/// Runs the `trogon-sandbox-policy` command line on the words after the
/// program name and returns the exit status.
#[cfg(windows)]
pub fn run_cli(operands: &[String], version: &str) -> i32 {
    match Invocation::parse(operands, version) {
        Invocation::Run(args) => windows_run_cmd::run(args),
        Invocation::Windows { uninstall, args } => windows_cmd::run(uninstall, &args),
        Invocation::Proxy(_) => print_err("Error: trogon-sandbox-policy proxy is not available on Windows\n"),
        Invocation::Print(text) => print_out(&text),
        Invocation::Fail(text) => print_err(&text),
    }
}

/// Runs `trogon-sandbox-policy-proxy` on the words after the program name and
/// returns the exit status.
#[cfg(unix)]
pub fn run_proxy(args: &[String]) -> i32 {
    proxy_cmd::run(args)
}

/// Runs `trogon-sandbox-policy-proxy` on the words after the program name and
/// returns the exit status.
#[cfg(windows)]
pub fn run_proxy(_args: &[String]) -> i32 {
    print_err("Error: trogon-sandbox-policy-proxy is not available on Windows\n")
}

#[cfg(any(unix, windows))]
fn print_out(text: &str) -> i32 {
    // A closed standard output leaves nowhere to report the text to.
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
    0
}

#[cfg(any(unix, windows))]
fn print_err(text: &str) -> i32 {
    // A closed standard error leaves nowhere to report the failure to.
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
    1
}
