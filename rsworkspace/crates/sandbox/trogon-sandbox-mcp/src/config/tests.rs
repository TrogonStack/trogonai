use clap::Parser;

use super::Config;

#[test]
fn a_tenant_is_all_that_is_required() -> Result<(), clap::Error> {
    let config = Config::try_parse_from(["trogon-sandbox-mcp", "--tenant", "acme"])?;
    assert_eq!(config.tenant.as_str(), "acme");
    assert_eq!(config.nats_url, "nats://localhost:4222");
    assert_eq!(config.log_level, "info");
    assert!(config.api_key.is_none());
    assert!(config.root.is_empty());
    Ok(())
}

#[test]
fn an_empty_tenant_is_refused() {
    assert!(Config::try_parse_from(["trogon-sandbox-mcp", "--tenant", ""]).is_err());
}

#[test]
fn lists_split_on_their_delimiters() -> Result<(), clap::Error> {
    let config = Config::try_parse_from([
        "trogon-sandbox-mcp",
        "--tenant",
        "acme",
        "--allow-tool",
        "a,b*",
        "--deny-command",
        "rm\nsudo",
        "--root",
        "/workspace,/tmp",
    ])?;
    assert_eq!(config.allow_tool, ["a", "b*"]);
    assert_eq!(config.deny_command, ["rm", "sudo"]);
    assert_eq!(config.root, ["/workspace", "/tmp"]);
    Ok(())
}
