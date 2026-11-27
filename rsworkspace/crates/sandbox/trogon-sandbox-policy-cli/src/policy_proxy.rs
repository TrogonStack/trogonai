//! The `trogon-sandbox-policy-proxy` command: serves the policy proxy without
//! running a sandboxed command.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(trogon_sandbox_policy_cli::run_proxy(&args));
}
