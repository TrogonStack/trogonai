#![cfg(feature = "cli")]

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use mcp_pack::{McpPack, McpPackSpec};

#[derive(Debug, Parser)]
#[command(name = "mcp-pack", about = "Build the first-party MCP policy bundle archive")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Assemble and sign the default pack tarball.
    Build {
        /// Output path for the uncompressed tar archive.
        #[arg(long, short = 'o', default_value = "./pack.bundle")]
        out: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build { out } => {
            let spec = McpPackSpec::with_ephemeral_signer();
            let pack = McpPack::new(spec);
            pack.build_to_path(&out)?;
            eprintln!("wrote signed bundle to {}", out.display());
        }
    }
    Ok(())
}
