//! `startup()` of `sdk.mjs` 0.3.323. Python 0.2.181 has no pre-warming, so the Rust API follows TypeScript.
//!
//! `startup` returns a [`WarmQuery`]: a process that finished its initialize handshake and waits for a prompt.

use std::time::Duration;

use crate::{
    ClaudeAgentOptions, ClaudeSdkClient, Error, Query, UserInput,
    adapters::{
        CurrentDirectory, EnvironmentVariable, RuntimeAdapters, SessionFileSystem, SpawnProcess, TemporaryDirectory,
    },
    driver::Completion,
    startup_timeout::{StartupHooksRunning, StartupSettings},
};

/// What [`startup`] takes: TypeScript's `{options, initializeTimeoutMs}`.
#[derive(Clone, Default)]
pub struct StartupOptions {
    pub options: ClaudeAgentOptions,
    /// The most time to wait for the process to initialize. `None` is 60 seconds, and for [`startup`] the time in
    /// which a SessionStart or Setup hook is reported running is then not counted, for at most 10 minutes. A value
    /// counts throughout.
    pub initialize_timeout: Option<Duration>,
    /// TypeScript `onStartupHooksRunning`: told how many reported startup hooks are still running.
    pub on_startup_hooks_running: Option<StartupHooksRunning>,
}

impl StartupOptions {
    pub fn new(options: ClaudeAgentOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }
}

/// A process that finished its initialize handshake and waits for a prompt.
pub struct WarmQuery {
    client: ClaudeSdkClient,
}

impl WarmQuery {
    /// TypeScript `WarmQuery.query(prompt)`. Taking `self` is how Rust says the call "can only be called once".
    pub async fn query(self, prompt: impl Into<UserInput>) -> Result<Query, Error> {
        crate::client::start_query(self.client, prompt.into(), Completion::Run).await
    }

    /// TypeScript `WarmQuery.close()`: discards the process without sending a prompt.
    pub fn close(self) {
        self.client.close();
    }

    /// TypeScript `WarmQuery[Symbol.asyncDispose]()`: closes and waits for the process to be cleaned up.
    pub async fn dispose(self) -> Result<(), Error> {
        self.client.disconnect().await
    }

    /// The initialize response the process answered with.
    pub async fn connection_info(&self) -> Result<crate::ConnectionInfo, Error> {
        self.client.handle().initialization_result().await
    }
}

/// TypeScript `startup()`, with the native process spawner.
pub async fn startup(params: StartupOptions) -> Result<WarmQuery, Error> {
    let adapters: RuntimeAdapters = RuntimeAdapters::default();
    start_warm(params, adapters, true).await
}

/// [`startup`] through caller-supplied adapters. The caller owns spawning, so no executable discovery runs.
pub async fn startup_with_adapters<E, P, F>(
    params: StartupOptions,
    adapters: RuntimeAdapters<E, P, F>,
) -> Result<WarmQuery, Error>
where
    E: EnvironmentVariable + CurrentDirectory<Error = std::io::Error> + TemporaryDirectory,
    P: SpawnProcess<Error = std::io::Error>,
    F: SessionFileSystem,
{
    start_warm(params, adapters, false).await
}

async fn start_warm<E, P, F>(
    params: StartupOptions,
    adapters: RuntimeAdapters<E, P, F>,
    discover: bool,
) -> Result<WarmQuery, Error>
where
    E: EnvironmentVariable + CurrentDirectory<Error = std::io::Error> + TemporaryDirectory,
    P: SpawnProcess<Error = std::io::Error>,
    F: SessionFileSystem,
{
    let StartupOptions {
        options,
        initialize_timeout,
        on_startup_hooks_running,
    } = params;
    let settings = StartupSettings {
        initialize_timeout,
        on_hooks_running: on_startup_hooks_running,
    };
    let client = ClaudeSdkClient::connect_startup(options, adapters, discover, settings).await?;
    Ok(WarmQuery { client })
}
