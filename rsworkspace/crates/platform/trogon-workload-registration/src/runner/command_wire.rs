use super::configuration_wire::ConfigurationWire;

#[derive(clap::Subcommand)]
pub(super) enum CommandWire {
    /// Create or verify the protected scope streams using a provisioner identity.
    Provision(ConfigurationWire),
    /// Publish fresh signed controller frames under one exclusive producer claim.
    Serve(ConfigurationWire),
    /// Apply one fresh signed complete denial using an emergency identity.
    Withdraw(ConfigurationWire),
}
