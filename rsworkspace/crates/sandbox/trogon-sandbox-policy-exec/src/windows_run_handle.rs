//! The caller's hold on a running Windows command: the live network policy
//! and a way to stop it.

use std::sync::Arc;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::NetworkPolicy;
use trogon_sandbox_egress_proxy::PolicyHandle;

use crate::config::PolicyExecConfig;
use crate::error::RuntimeError;

enum Serving {
    Starting,
    Proxy(PolicyHandle),
    Finished,
}

struct Shared {
    serving: watch::Sender<Serving>,
    stop: CancellationToken,
}

/// Cloneable. Pass one in `WindowsRun::handle`, keep a clone, and call it while
/// `run_on_windows` is awaited. Only the network policy is live on Windows;
/// the file rules, credentials and the rest are fixed for the command.
#[derive(Clone)]
pub struct WindowsRunHandle {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for WindowsRunHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WindowsRunHandle")
    }
}

impl Default for WindowsRunHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsRunHandle {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                serving: watch::channel(Serving::Starting).0,
                stop: CancellationToken::new(),
            }),
        }
    }

    /// Replaces the network policy the running command is held to, waiting
    /// for the proxy to come up when the run has not reached it yet. The rest
    /// of `config` is validated but not applied: it cannot change for a
    /// command that is already set up.
    pub async fn update_config(&self, config: PolicyExecConfig) -> Result<(), RuntimeError> {
        config.validate()?;
        self.update_network(config.policy().network.clone()).await
    }

    pub(crate) async fn update_network(&self, network: NetworkPolicy) -> Result<(), RuntimeError> {
        let mut serving = self.shared.serving.subscribe();
        let policy = loop {
            match &*serving.borrow_and_update() {
                Serving::Proxy(policy) => break policy.clone(),
                Serving::Finished => return Err(not_running()),
                Serving::Starting => {}
            }
            serving.changed().await.map_err(|_| not_running())?;
        };
        policy.replace(network).await.map_err(RuntimeError::PolicyRejectedError)
    }

    /// Stops the command, before it starts too. The run then ends with exit
    /// code 0, as an interrupt does.
    pub fn terminate(&self) {
        self.shared.stop.cancel();
    }

    pub(crate) async fn terminated(&self) {
        self.shared.stop.cancelled().await;
    }

    pub(crate) fn serve(&self, policy: PolicyHandle) {
        self.shared.serving.send_replace(Serving::Proxy(policy));
    }

    pub(crate) fn finish_on_drop(&self) -> Finished {
        Finished(self.clone())
    }
}

pub(crate) struct Finished(WindowsRunHandle);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.shared.serving.send_replace(Serving::Finished);
    }
}

fn not_running() -> RuntimeError {
    RuntimeError::Unsupported("the Windows command is not running".to_string())
}

#[cfg(test)]
mod tests;
