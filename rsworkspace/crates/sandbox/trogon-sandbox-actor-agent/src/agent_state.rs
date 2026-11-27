use std::sync::Arc;

use crate::{
    ActorIdentitySource, BindingState, EntropyReseeder, ExecRegistry, KernelEntropyReseeder,
    MIN_SUPPORTED_WORKER_PROTOCOL_VERSION, ReverseProxy, TerminalSessions,
};

/// Everything the agent's request handlers share.
pub struct AgentState {
    pub(crate) binding: BindingState,
    pub(crate) executions: ExecRegistry,
    pub(crate) identity: ActorIdentitySource,
    pub(crate) proxy: ReverseProxy,
    pub(crate) entropy_reseeder: Arc<dyn EntropyReseeder>,
    pub(crate) min_worker_protocol_version: u32,
    pub(crate) terminals: Arc<TerminalSessions>,
}

impl AgentState {
    pub fn new(identity: ActorIdentitySource) -> Self {
        Self {
            binding: BindingState::new(),
            executions: ExecRegistry::new(),
            identity,
            proxy: ReverseProxy::new(),
            entropy_reseeder: Arc::new(KernelEntropyReseeder),
            min_worker_protocol_version: MIN_SUPPORTED_WORKER_PROTOCOL_VERSION,
            terminals: TerminalSessions::new(),
        }
    }

    /// Refuses workers older than `version` on reserved paths.
    pub fn with_min_worker_protocol_version(mut self, version: u32) -> Self {
        self.min_worker_protocol_version = version;
        self
    }

    /// Replaces how a bind's entropy seed reaches the kernel.
    pub fn with_entropy_reseeder(mut self, reseeder: Arc<dyn EntropyReseeder>) -> Self {
        self.entropy_reseeder = reseeder;
        self
    }
}
