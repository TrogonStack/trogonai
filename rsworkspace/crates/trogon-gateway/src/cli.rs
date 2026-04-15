use trogon_service_config::RuntimeConfigArgs;

#[derive(clap::Parser, Clone)]
#[command(name = "trogon-gateway", about = "Unified gateway ingestion binary")]
pub struct Cli {
    #[command(flatten)]
    pub runtime: RuntimeConfigArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand, Clone)]
pub enum Command {
    Serve,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_with_no_config_is_constructible() {
        let cmd = Command::Serve { config: None };
        assert!(matches!(cmd, Command::Serve { config: None }));
    }

    #[test]
    fn serve_with_config_path_is_constructible() {
        let path = PathBuf::from("/etc/trogon/gateway.toml");
        let cmd = Command::Serve {
            config: Some(path.clone()),
        };
        assert!(matches!(cmd, Command::Serve { config: Some(p) } if p == path));
    }

    #[test]
    fn cli_stores_command() {
        let cli = Cli {
            command: Command::Serve { config: None },
        };
        assert!(matches!(cli.command, Command::Serve { config: None }));
    }

    #[test]
    fn command_is_cloneable() {
        let cmd = Command::Serve {
            config: Some(PathBuf::from("/tmp/cfg.toml")),
        };
        let cloned = cmd.clone();
        assert!(
            matches!(cloned, Command::Serve { config: Some(ref p) } if p == &PathBuf::from("/tmp/cfg.toml"))
        );
    }
}
