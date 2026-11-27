use std::str::FromStr;

use crate::SshCommandError;
use crate::constants::DEFAULT_PORT;

/// Where an `ssh [-p PORT] [user@]host` command line connects.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SshCommand {
    pub user: Option<String>,
    pub host: String,
    pub port: u16,
}

impl FromStr for SshCommand {
    type Err = SshCommandError;

    fn from_str(command: &str) -> Result<Self, Self::Err> {
        let mut words = command.split_whitespace();
        if words.next() != Some("ssh") {
            return Err(SshCommandError::NotSsh);
        }
        let mut port = DEFAULT_PORT;
        let mut destination = None;
        while let Some(word) = words.next() {
            if word == "-o" {
                words.next();
            } else if word == "-p" {
                port = parse_port(words.next().ok_or(SshCommandError::MissingPort)?)?;
            } else if let Some(value) = word.strip_prefix("-p") {
                port = parse_port(value)?;
            } else if !word.starts_with('-') && destination.is_none() {
                destination = Some(word);
            }
        }
        let destination = destination.ok_or(SshCommandError::MissingHost)?;
        match destination.rsplit_once('@') {
            Some((_, "")) => Err(SshCommandError::MissingHost),
            Some((user, host)) => Ok(Self {
                user: Some(user.to_owned()),
                host: host.to_owned(),
                port,
            }),
            None => Ok(Self {
                user: None,
                host: destination.to_owned(),
                port,
            }),
        }
    }
}

fn parse_port(value: &str) -> Result<u16, SshCommandError> {
    value
        .parse()
        .map_err(|_| SshCommandError::InvalidPort(value.to_owned()))
}

#[cfg(test)]
mod tests;
