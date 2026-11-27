use std::io::Write;

#[cfg(target_os = "linux")]
fn main() {
    match trogon_sandbox_policy_exec::run_helper_if_invoked() {
        Some(code) => std::process::exit(code),
        None => refuse("trogon-sandbox-policy-exec-helper runs only as the sandbox's own helper process"),
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    refuse("trogon-sandbox-policy-exec-helper is only used on Linux");
}

fn refuse(message: &str) -> ! {
    // The process is exiting with a failure code; a closed stderr has nowhere better to report to.
    let _ = writeln!(std::io::stderr(), "{message}");
    std::process::exit(2);
}
