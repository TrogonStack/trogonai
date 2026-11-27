use std::path::PathBuf;
use std::time::Duration;

use crate::{MaxLeases, SpaceConfigError, SpaceTemplate};

/// The unchecked settings of a [`SpaceConfig`].
#[derive(Debug, Clone)]
pub struct SpaceConfigParts {
    /// The template every lease is created from.
    pub template: SpaceTemplate,
    /// The directory holding the worker's lock and dispatch ledger.
    pub state_dir: PathBuf,
    /// How long a create may take from request to a sandbox that answers process queries.
    pub boot_timeout: Duration,
    /// How long pause, suspend, wake and delete may take.
    pub operation_timeout: Duration,
    /// How often an operation or sandbox state is polled.
    pub poll_interval: Duration,
    /// How many leases the worker holds at once.
    pub max_leases: MaxLeases,
    /// How long past its own timeout a command is waited for, which covers SPACE
    /// enforcing the timeout and sending the exit event.
    pub command_grace: Duration,
    /// How long reading one file through the guest may take.
    pub file_read_timeout: Duration,
}

/// The settings of a SPACE worker, with every duration that must be positive checked.
#[derive(Debug, Clone)]
pub struct SpaceConfig {
    pub(crate) template: SpaceTemplate,
    pub(crate) state_dir: PathBuf,
    pub(crate) boot_timeout: Duration,
    pub(crate) operation_timeout: Duration,
    pub(crate) poll_interval: Duration,
    pub(crate) max_leases: MaxLeases,
    pub(crate) command_grace: Duration,
    pub(crate) file_read_timeout: Duration,
}

impl SpaceConfig {
    /// Checks the settings.
    pub fn new(parts: SpaceConfigParts) -> Result<Self, SpaceConfigError> {
        for (name, value) in [
            ("boot timeout", parts.boot_timeout),
            ("operation timeout", parts.operation_timeout),
            ("poll interval", parts.poll_interval),
            ("file read timeout", parts.file_read_timeout),
        ] {
            if value.is_zero() {
                return Err(SpaceConfigError::ZeroDuration(name));
            }
        }
        Ok(Self {
            template: parts.template,
            state_dir: parts.state_dir,
            boot_timeout: parts.boot_timeout,
            operation_timeout: parts.operation_timeout,
            poll_interval: parts.poll_interval,
            max_leases: parts.max_leases,
            command_grace: parts.command_grace,
            file_read_timeout: parts.file_read_timeout,
        })
    }
}
