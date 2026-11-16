use clap::Subcommand;

/// What the `config` command does.
#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum ConfigCommand {
    /// Prints every setting with its effective value and its source.
    #[command(
        long_about = "Prints every setting with its effective value and the source it comes from: the \
            built-in default, the configuration file, the environment, or a flag (ADR#0007).\n\n\
            The configuration is resolved and validated the way startup does it, and refused with \
            the same error. A secret is shown only by its env://NAME or file:///path reference, a \
            secret given inline is shown as <redacted>, and so are the credentials a NATS server \
            address carries. Nothing a setting names is read, and nothing is connected to."
    )]
    Dump {
        /// Shows the values after every source is merged. Required.
        #[arg(long, required = true)]
        resolved: bool,
    },
}
