use std::time::Duration;

use serde_json::Value;

use crate::configuration_provider::ConfigurationProvider;
use crate::constants::DEFAULT_REQUEST_TIMEOUT;

#[derive(Clone)]
pub struct LspServerOptions {
    pub request_timeout: Duration,
    /// Time to wait for the server to exit on its own before it is signalled.
    pub stop_timeout: Duration,
    /// Time to wait after SIGTERM before SIGKILL.
    pub kill_grace: Duration,
    /// How often the process is polled to detect that it exited.
    pub exit_poll_interval: Duration,
    /// Merged over the default client capabilities.
    pub capabilities: Option<Value>,
    /// Answers `workspace/configuration` items. Defaults to `null` for every item.
    pub configuration: Option<ConfigurationProvider>,
}

impl Default for LspServerOptions {
    fn default() -> Self {
        Self {
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            stop_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_secs(2),
            exit_poll_interval: Duration::from_secs(1),
            capabilities: None,
            configuration: None,
        }
    }
}
