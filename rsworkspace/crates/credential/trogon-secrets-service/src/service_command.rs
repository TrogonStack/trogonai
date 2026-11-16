use clap::Subcommand;

use super::ConfigCommand;

/// What the secrets service does other than serve releases, which it does
/// when no command is given.
#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum ServiceCommand {
    /// Inspects the configuration the service runs with.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}
