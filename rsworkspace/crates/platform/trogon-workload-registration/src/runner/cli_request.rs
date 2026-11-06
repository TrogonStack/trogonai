use super::command_wire::CommandWire;
use super::configuration_wire::ConfigurationWire;
use super::operation::Operation;

#[derive(clap::Parser)]
#[command(version, about = "Protected scoped registration administration")]
pub(super) struct CliRequest {
    #[command(subcommand)]
    command: CommandWire,
}

impl CliRequest {
    pub fn into_parts(self) -> (Operation, ConfigurationWire) {
        match self.command {
            CommandWire::Provision(wire) => (Operation::Provision, wire),
            CommandWire::Serve(wire) => (Operation::Serve, wire),
            CommandWire::Withdraw(wire) => (Operation::Withdraw, wire),
        }
    }
}
