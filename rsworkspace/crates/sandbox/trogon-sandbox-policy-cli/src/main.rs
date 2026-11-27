//! The `trogon-sandbox-policy` command: runs one command under a settings file.

fn main() {
    #[cfg(target_os = "linux")]
    if let Some(code) = trogon_sandbox_policy_exec::run_helper_if_invoked() {
        std::process::exit(code);
    }
    let operands: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    #[cfg(windows)]
    if operands.first().map(String::as_str) == Some(trogon_sandbox_windows_fence::DISPATCH_ARG1) {
        let argv0 = std::env::args().next().unwrap_or_default();
        let rest = std::iter::once(argv0).chain(operands.iter().skip(1).cloned());
        std::process::exit(trogon_sandbox_fence_helper::run_from_args(rest));
    }
    std::process::exit(trogon_sandbox_policy_cli::run_cli(&operands, env!("CARGO_PKG_VERSION")));
}
