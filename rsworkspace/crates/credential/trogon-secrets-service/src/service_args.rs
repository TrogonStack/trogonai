use clap::Parser;
use trogon_service_config::RuntimeConfigArgs;

/// The command line of the secrets service: the configuration file, and the
/// NATS connection flags every service here takes.
#[derive(Clone, Debug, Default, Parser)]
#[command(
    name = "trogon-secrets-service",
    about = "Secrets service: releases credentials to runtime services over NATS",
    long_about = "Secrets service: releases credentials to runtime services over NATS.\n\n\
        Each setting is read from the configuration file named by --config, then from its \
        environment variable, then from its flag where it has one, each over the one before; an \
        empty value counts as unset. The file holds no secret: a secret it names is an \
        env://NAME or file:///path reference. The sealing key seed is such a reference wherever \
        it is set, and the AppRole secret id is read from the file that holds it. Only \
        NATS_TLS_DISABLED and OPENBAO_TLS_DISABLED, read from the environment alone, disable TLS.\n\n\
        At startup the service reads the tenant owners file and the release policies file once, \
        reads its sealing key, connects to NATS, and logs in to OpenBao before it joins the \
        release queue group. It does not start when a setting or a file is missing or invalid."
)]
pub struct ServiceArgs {
    #[command(flatten)]
    pub runtime: RuntimeConfigArgs,
}
