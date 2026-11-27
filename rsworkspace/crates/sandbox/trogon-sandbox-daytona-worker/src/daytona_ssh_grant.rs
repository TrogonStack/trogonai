use std::fmt;

use chrono::DateTime;
use serde::Deserialize;

use crate::daytona_ssh_grant_wire::DaytonaSshGrantWire;
use crate::{DaytonaSshGrantError, SshCommand};

/// Access to a sandbox through Daytona's own SSH gateway: where to connect and the token that opens it.
///
/// `Debug` never shows the token or the command line, which carries it.
#[derive(Clone, Eq, PartialEq, Deserialize)]
#[serde(try_from = "DaytonaSshGrantWire")]
pub struct DaytonaSshGrant {
    pub host: String,
    pub port: u16,
    /// The SSH user name, which is the access token.
    pub token: String,
    /// The `ssh` command line Daytona suggests.
    pub command: String,
    /// When the token stops working, in unix seconds.
    pub expires_at: Option<u64>,
}

impl TryFrom<DaytonaSshGrantWire> for DaytonaSshGrant {
    type Error = DaytonaSshGrantError;

    fn try_from(wire: DaytonaSshGrantWire) -> Result<Self, Self::Error> {
        let destination: SshCommand = wire.ssh_command.parse()?;
        let expires_at = wire
            .expires_at
            .map(|stamp| {
                DateTime::parse_from_rfc3339(&stamp)
                    .map_err(|_| DaytonaSshGrantError::Expiry)
                    .map(|moment| u64::try_from(moment.timestamp()).unwrap_or(0))
            })
            .transpose()?;
        Ok(Self {
            host: destination.host,
            port: destination.port,
            token: destination.user.unwrap_or(wire.token),
            command: wire.ssh_command,
            expires_at,
        })
    }
}

impl fmt::Debug for DaytonaSshGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DaytonaSshGrant")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("token", &"[REDACTED]")
            .field("command", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[cfg(test)]
mod tests;
