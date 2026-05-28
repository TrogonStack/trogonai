mod mcp;
mod traffic;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use trogon_agent_registry_controller::{GitSyncConfig, validate_repo_sync};

#[derive(Parser, Debug)]
#[command(name = "agctl", about = "Agent gateway control utilities")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Registry(RegistryCommand),
    Traffic(traffic::TrafficCommand),
    Mcp(mcp::McpCommand),
}

#[derive(Args, Debug)]
struct RegistryCommand {
    #[command(subcommand)]
    command: RegistrySubcommand,
}

#[derive(Subcommand, Debug)]
enum RegistrySubcommand {
    /// Validate signed agent manifests in a Git working tree before commit.
    Sync(SyncArgs),
}

#[derive(Args, Debug)]
struct SyncArgs {
    #[arg(long)]
    repo: PathBuf,

    #[arg(long, default_value = "agents")]
    agents_dir: PathBuf,

    #[arg(long, help = "Ed25519 public or private key PEM used to verify manifest signatures")]
    verify_key: PathBuf,

    #[arg(long, help = "Optional Ed25519 private key PEM to re-sign manifests after validation")]
    signer_key: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Registry(registry) => match registry.command {
            RegistrySubcommand::Sync(args) => run_registry_sync(args),
        },
        Command::Traffic(traffic) => traffic::run(traffic).await,
        Command::Mcp(mcp) => mcp::run(mcp),
    }
}

fn run_registry_sync(args: SyncArgs) -> ExitCode {
    let verify_pem = match std::fs::read_to_string(&args.verify_key) {
        Ok(pem) => pem,
        Err(error) => {
            eprintln!("read verify key {}: {error}", args.verify_key.display());
            return ExitCode::FAILURE;
        }
    };

    let config = GitSyncConfig {
        repo_path: args.repo,
        agents_dir: args.agents_dir,
        git_remote: None,
        git_ref: "main".into(),
    };

    match validate_repo_sync(config, &verify_pem, args.signer_key.as_deref()) {
        Ok(report) => {
            println!(
                "ok: validated {} manifest(s) at git commit {}",
                report.manifests_seen, report.git_commit
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("registry sync failed: {error}");
            ExitCode::FAILURE
        }
    }
}
