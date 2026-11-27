use clap::Parser;

use super::Args;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn required_args_parse_successfully() -> TestResult {
    let args = Args::try_parse_from([
        "trogon-sandbox-subprocess-worker",
        "--agent-binary",
        "/usr/local/bin/sandbox-agent",
        "--workspace-root",
        "/tmp/workspaces",
    ])?;
    assert_eq!(args.agent_binary, "/usr/local/bin/sandbox-agent");
    assert_eq!(args.workspace_root, std::path::PathBuf::from("/tmp/workspaces"));
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert_eq!(args.max_concurrent_leases, 4);
    Ok(())
}

#[test]
fn missing_required_agent_binary_fails_to_parse() {
    let result = Args::try_parse_from(["trogon-sandbox-subprocess-worker", "--workspace-root", "/tmp/ws"]);
    assert!(result.is_err());
}

#[test]
fn missing_required_workspace_root_fails_to_parse() {
    let result = Args::try_parse_from([
        "trogon-sandbox-subprocess-worker",
        "--agent-binary",
        "/usr/local/bin/agent",
    ]);
    assert!(result.is_err());
}
