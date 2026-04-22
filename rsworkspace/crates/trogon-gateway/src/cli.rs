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
    fn serve_command_is_constructible() {
        let cmd = Command::Serve;
        assert!(matches!(cmd, Command::Serve));
    }

    #[test]
    fn command_is_cloneable() {
        let cloned = Command::Serve.clone();
        assert!(matches!(cloned, Command::Serve));
    }
}
