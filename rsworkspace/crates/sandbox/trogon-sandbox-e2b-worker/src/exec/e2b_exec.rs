use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use async_nats::jetstream;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{ExecutionId, LeaseId};
use trogon_sandbox_file_ops::Tracking;
use trogon_sandbox_protocol::DispatchVerifier;

use super::command_custody::CommandCustody;
use super::command_store::CommandStore;
use super::exec_error::ExecError;
use super::execution::Execution;
use crate::envd::{Envd, EnvdTarget};
use crate::watch::Watches;

/// Per-lease operations (run, poll, kill, files, domains) over envd.
///
/// Each `handle_*` method takes the wire request and returns the wire response, so the NATS handlers stay
/// thin. Options envd cannot serve are refused with the wire's typed outcomes instead of being dropped.
///
/// envd's `Start` call is a stream tied to one request, so a detached command is a small launcher that
/// backgrounds the real command with its output and exit status redirected into files under
/// `/tmp/.trogon-exec-<id>`. Poll reads those files, kill signals the recorded process group.
pub struct E2bExec {
    pub(super) envd: Envd,
    pub(super) executions: Mutex<HashMap<(LeaseId, ExecutionId), Execution>>,
    pub(super) execution_gates: Mutex<HashMap<LeaseId, Arc<Mutex<()>>>>,
    pub(super) watches: Watches,
    pub(super) tracking: Tracking,
    pub(super) command_custody: Box<dyn CommandStore>,
    pub(super) worker_fence: OnceLock<&'static DispatchVerifier>,
}

impl E2bExec {
    pub fn new(envd: Envd) -> Self {
        Self {
            watches: Watches::new(envd.clone()),
            envd,
            executions: Mutex::new(HashMap::new()),
            execution_gates: Mutex::new(HashMap::new()),
            tracking: Tracking::new(),
            command_custody: Box::new(CommandCustody::default()),
            worker_fence: OnceLock::new(),
        }
    }

    /// Keeps keyed command launches durable in JetStream.
    pub fn configure_command_launches(&self, context: jetstream::Context) -> Result<(), ExecError> {
        self.command_custody.configure(context)
    }

    /// Keeps keyed command launches durable and refuses fresh ones until the dispatch fence is trustworthy.
    pub fn configure_worker_command_launches(
        &self,
        context: jetstream::Context,
        verifier: &'static DispatchVerifier,
    ) -> Result<(), ExecError> {
        self.worker_fence
            .set(verifier)
            .map_err(|_| ExecError::WorkerFenceAlreadyConfigured)?;
        self.configure_command_launches(context)
    }

    /// Publishes file watch overflows on the lease evidence timeline.
    pub fn with_evidence(mut self, js: jetstream::Context) -> Self {
        self.watches = self.watches.with_evidence(js);
        self
    }

    /// Replaces the watch registry, for tests that need a fast poll interval.
    pub fn with_watches(mut self, watches: Watches) -> Self {
        self.watches = watches;
        self
    }

    /// Removes a released lease's watchers from envd and forgets them.
    pub async fn release_watches(&self, lease_id: &str, target: &EnvdTarget) {
        self.watches.release(lease_id, target).await;
    }

    /// Records the lease-start baseline `changes` diffs against. Best effort.
    pub async fn record_start_baseline(&self, lease_id: &str, target: &EnvdTarget) {
        crate::track::record_start_baseline(&self.tracking, lease_id, &self.envd, target).await;
    }
}
