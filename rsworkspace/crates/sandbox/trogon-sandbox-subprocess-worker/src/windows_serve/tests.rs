use super::*;

#[test]
fn required_args_parse_successfully() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::try_parse_from([
        "sandbox-subprocess-worker",
        "--agent-binary",
        r"C:\Program Files\agent\agent.exe",
        "--workspace-root",
        r"C:\sandbox\workspaces",
        "--agent-arg=--serve",
    ])?;
    assert_eq!(args.agent_args, vec!["--serve".to_string()]);
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert_eq!(args.max_concurrent_leases, 4);
    Ok(())
}

#[test]
fn missing_required_args_fail_to_parse() -> Result<(), Box<dyn std::error::Error>> {
    assert!(Args::try_parse_from(["sandbox-subprocess-worker"]).is_err());
    Ok(())
}
