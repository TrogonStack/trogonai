use std::net::IpAddr;
use std::path::PathBuf;

#[derive(clap::Parser, Debug)]
#[command(name = "trogon-provider-gateway", version, about = "Provider gateway HTTP server")]
pub(crate) struct Cli {
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,

    #[arg(long)]
    pub(crate) host: Option<IpAddr>,

    #[arg(long)]
    pub(crate) port: Option<u16>,
}

#[cfg(test)]
mod tests;
