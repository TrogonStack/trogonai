use std::process::ExitCode;

use clap::{Args, Subcommand, ValueEnum};

const NOT_YET_IMPLEMENTED: &str = "not yet implemented";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Args, Debug)]
pub struct OutputArgs {
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub output: OutputFormat,
}

#[derive(Args, Debug)]
pub struct McpCommand {
    #[command(subcommand)]
    pub command: McpSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum McpSubcommand {
    /// Inspect registered MCP servers.
    Servers(ServersCommand),
    /// Inspect gateway policy bundles.
    Policies(PoliciesCommand),
    /// Stream gateway audit events.
    Audit(AuditCommand),
    /// Report MCP gateway health.
    Health(OutputArgs),
}

#[derive(Args, Debug)]
pub struct ServersCommand {
    #[command(subcommand)]
    pub command: ServersSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ServersSubcommand {
    /// List registered MCP servers.
    List(OutputArgs),
    /// Show one server's record.
    Get(GetServerArgs),
}

#[derive(Args, Debug)]
pub struct GetServerArgs {
    /// MCP server identifier.
    pub server_id: String,

    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Args, Debug)]
pub struct PoliciesCommand {
    #[command(subcommand)]
    pub command: PoliciesSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum PoliciesSubcommand {
    /// List policy bundles in the KV store.
    List(OutputArgs),
    /// Show the policy applied to a server.
    Get(GetPolicyArgs),
}

#[derive(Args, Debug)]
pub struct GetPolicyArgs {
    /// MCP server identifier.
    pub server_id: String,

    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Args, Debug)]
pub struct AuditCommand {
    #[command(subcommand)]
    pub command: AuditSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum AuditSubcommand {
    /// Subscribe to gateway audit subjects and print one envelope per line.
    Tail(OutputArgs),
}

pub fn run(mcp: McpCommand) -> ExitCode {
    match mcp.command {
        McpSubcommand::Health(args) => run_health(args),
        McpSubcommand::Servers(servers) => match servers.command {
            ServersSubcommand::List(_) | ServersSubcommand::Get(_) => run_unimplemented(),
        },
        McpSubcommand::Policies(policies) => match policies.command {
            PoliciesSubcommand::List(_) | PoliciesSubcommand::Get(_) => run_unimplemented(),
        },
        McpSubcommand::Audit(audit) => match audit.command {
            AuditSubcommand::Tail(_) => run_unimplemented(),
        },
    }
}

fn run_health(args: OutputArgs) -> ExitCode {
    match args.output {
        OutputFormat::Text => println!("ok"),
        OutputFormat::Json => println!(r#"{{"status":"ok"}}"#),
    }
    ExitCode::SUCCESS
}

fn run_unimplemented() -> ExitCode {
    eprintln!("{NOT_YET_IMPLEMENTED}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[derive(Parser, Debug)]
    #[command(name = "agctl")]
    struct TestCli {
        #[command(subcommand)]
        command: TestCommand,
    }

    #[derive(Subcommand, Debug)]
    enum TestCommand {
        Mcp(McpCommand),
    }

    fn parse_mcp(args: &[&str]) -> McpCommand {
        let cli = TestCli::try_parse_from(std::iter::once("agctl").chain(args.iter().copied()))
            .expect("parse");
        let TestCommand::Mcp(mcp) = cli.command;
        mcp
    }

    #[test]
    fn command_tree_debug_assert() {
        TestCli::command().debug_assert();
    }

    #[test]
    fn health_text_returns_success() {
        let mcp = parse_mcp(&["mcp", "health"]);
        assert_eq!(run(mcp), ExitCode::SUCCESS);
    }

    #[test]
    fn health_json_returns_success() {
        let mcp = parse_mcp(&["mcp", "health", "--output", "json"]);
        assert_eq!(run(mcp), ExitCode::SUCCESS);
    }

    #[test]
    fn servers_list_returns_unimplemented_exit() {
        let mcp = parse_mcp(&["mcp", "servers", "list"]);
        assert_eq!(run(mcp), ExitCode::from(2));
    }

    #[test]
    fn servers_get_returns_unimplemented_exit() {
        let mcp = parse_mcp(&["mcp", "servers", "get", "demo-server"]);
        assert_eq!(run(mcp), ExitCode::from(2));
    }

    #[test]
    fn policies_list_returns_unimplemented_exit() {
        let mcp = parse_mcp(&["mcp", "policies", "list"]);
        assert_eq!(run(mcp), ExitCode::from(2));
    }

    #[test]
    fn policies_get_returns_unimplemented_exit() {
        let mcp = parse_mcp(&["mcp", "policies", "get", "demo-server"]);
        assert_eq!(run(mcp), ExitCode::from(2));
    }

    #[test]
    fn audit_tail_returns_unimplemented_exit() {
        let mcp = parse_mcp(&["mcp", "audit", "tail"]);
        assert_eq!(run(mcp), ExitCode::from(2));
    }
}
