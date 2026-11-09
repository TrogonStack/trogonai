use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use trogon_std::env::ReadEnv;

use crate::cli::Cli;
use crate::constants::{HOST_ENV, PORT_ENV};

#[derive(Debug)]
pub(crate) struct Config {
    bind_addr: SocketAddr,
}

impl Config {
    pub(crate) fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }
}

#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigInput {
    host: Option<String>,
    port: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("failed to read config file {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid TOML config file {path}")]
    ParseFile {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to read environment variable {variable}")]
    Environment {
        variable: &'static str,
        #[source]
        source: std::env::VarError,
    },
    #[error("host must be an IPv4 or IPv6 address")]
    Host(#[source] std::net::AddrParseError),
    #[error("port must be an integer from 0 to 65535")]
    Port(#[source] std::num::ParseIntError),
    #[error("port must be an integer from 0 to 65535")]
    FilePort(#[source] std::num::TryFromIntError),
}

pub(crate) fn load(cli: &Cli, env: &impl ReadEnv) -> Result<Config, ConfigError> {
    let input = if let Some(path) = &cli.config {
        let content = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
            path: path.clone(),
            source,
        })?;
        toml::from_str::<ConfigInput>(&content).map_err(|source| ConfigError::ParseFile {
            path: path.clone(),
            source,
        })?
    } else {
        ConfigInput::default()
    };

    let host = match cli.host {
        Some(host) => host,
        None => match read_env(env, HOST_ENV)?.or(input.host) {
            Some(host) => host.parse().map_err(ConfigError::Host)?,
            None => IpAddr::V4(Ipv4Addr::LOCALHOST),
        },
    };
    let port = match cli.port {
        Some(port) => port,
        None => match read_env(env, PORT_ENV)? {
            Some(port) => port.parse().map_err(ConfigError::Port)?,
            None => match input.port {
                Some(port) => u16::try_from(port).map_err(ConfigError::FilePort)?,
                None => 8317,
            },
        },
    };

    Ok(Config {
        bind_addr: SocketAddr::new(host, port),
    })
}

fn read_env(env: &impl ReadEnv, variable: &'static str) -> Result<Option<String>, ConfigError> {
    match env.var(variable) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(source) => Err(ConfigError::Environment { variable, source }),
    }
}

#[cfg(test)]
mod tests;
