//! Runs a shell command with no network except hosts the callback allows.
//!
//! `cargo run -p trogon-sandbox-policy-exec --example run_command -- 'curl -sS https://example.com'`

use std::io::Write;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use trogon_sandbox_policy_exec::{
    AllowRange, AskQuestion, AskVerdict, CommandSpec, DenialReason, NetworkPolicy, PolicyExec, PolicyExecConfig,
};

#[cfg(target_os = "linux")]
fn main() {
    if let Some(code) = trogon_sandbox_policy_exec::run_helper_if_invoked() {
        std::process::exit(code);
    }
    run();
}

#[cfg(target_os = "macos")]
fn main() {
    run();
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() {
    fail("trogon-sandbox-policy-exec supports Linux and macOS only");
}

fn fail(message: &str) -> ! {
    // The process is exiting with a failure code; a closed stderr has nowhere better to report to.
    let _ = writeln!(std::io::stderr(), "{message}");
    std::process::exit(2);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run() {
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => fail(&format!("tokio runtime: {error}")),
    };
    match runtime.block_on(execute()) {
        Ok(code) => std::process::exit(code),
        Err(error) => fail(&error.to_string()),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn execute() -> Result<i32, Box<dyn std::error::Error>> {
    let script = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "curl -sS https://example.com | head -c 200".to_string());
    let workspace = std::env::current_dir()?;

    let config = PolicyExecConfig::new(&workspace).network(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("0.0.0.0/0")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    });
    let runtime = PolicyExec::builder(config)
        .ask_callback(|question: AskQuestion| async move {
            if question.host.to_string().ends_with("example.com") {
                AskVerdict::Allow
            } else {
                AskVerdict::Deny(DenialReason::new("only example.com is allowed"))
            }
        })
        .build()?;

    let output = runtime.run(CommandSpec::shell(script)).await?;
    let mut stdout = std::io::stdout();
    write!(stdout, "{}", output.stdout_text())?;
    writeln!(
        std::io::stderr(),
        "{}",
        runtime.annotate_stderr(&output.command, &output.stderr_text())
    )?;
    runtime.reset().await;
    Ok(output.status.code().unwrap_or(1))
}
