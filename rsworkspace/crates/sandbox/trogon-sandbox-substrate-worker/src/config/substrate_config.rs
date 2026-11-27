use std::time::Duration;

use trogon_sandbox_contract::SubstrateSizeClasses;
use trogon_sandbox_protocol::select_substrate_size_class;

use super::{RouterEndpoint, SIZE_CLASS_LABEL, SandboxClass, SubstrateConfigError};
use crate::proto;
use crate::resources::{ActorResources, validate_image_digest};
use crate::router_tls::RouterTlsConfig;

#[derive(Debug, Clone)]
pub struct SubstrateConfig {
    pub ate_api_endpoint: String,
    pub router_endpoint: RouterEndpoint,
    pub router_tls: Option<RouterTlsConfig>,
    pub actor_cpu_millicores: u32,
    pub agent_image: String,
    pub agent_port: u16,
    pub sandbox_class: SandboxClass,
    pub snapshots_location: String,
    pub size_classes: SubstrateSizeClasses,
    pub template_build_timeout: Duration,
    pub resume_timeout: Duration,
    pub max_concurrent_leases: u32,
}

impl SubstrateConfig {
    pub fn resources_for(&self, memory_mib: u32) -> Result<ActorResources, SubstrateConfigError> {
        let class = select_substrate_size_class(&self.size_classes, memory_mib)
            .ok_or(SubstrateConfigError::MemoryExceedsSizeClasses)?;
        Ok(ActorResources::new(self.actor_cpu_millicores, class.memory_mib())?)
    }

    pub fn validate(&self) -> Result<(), SubstrateConfigError> {
        validate_image_digest(&self.agent_image)?;
        if self.agent_port == 0 {
            return Err(SubstrateConfigError::AgentPortZero);
        }
        for class in self.size_classes.as_slice() {
            ActorResources::new(self.actor_cpu_millicores, class.memory_mib())?;
        }
        Ok(())
    }

    /// The `worker_selector` for a lease asking for `memory_mib`.
    ///
    /// `None` when no configured class is big enough. The broker rejects that case before
    /// dispatch, but a worker must never assume the broker did its job, so the caller
    /// converts `None` into a refusal rather than into an unconstrained selector: an empty
    /// selector would schedule the lease onto whichever pool had room, which is exactly the
    /// silent under-provisioning the size-class table exists to prevent.
    pub fn size_selector(&self, memory_mib: u32) -> Option<proto::Selector> {
        let label = self.size_class_label(memory_mib)?;
        Some(proto::Selector {
            match_labels: [(SIZE_CLASS_LABEL.to_string(), label.to_string())].into(),
        })
    }

    /// The `ate.dev/size` label value a lease asking for `memory_mib` lands on.
    ///
    /// The golden build for a lease's template must land on a worker carrying this same
    /// label, or the actor's own `worker_selector` and the template's would ask for pools
    /// that cannot both be true of one worker. `TemplateKey` folds this in for exactly that
    /// reason: one image can have a different golden snapshot per size class.
    pub fn size_class_label(&self, memory_mib: u32) -> Option<&str> {
        select_substrate_size_class(&self.size_classes, memory_mib).map(|c| c.label())
    }

    /// The actor's public L7 endpoint, as `GetSandboxDomain` reports it.
    ///
    /// Every actor shares this one endpoint: the router selects among them by the
    /// `ate-target-actor` header on a request, not by a per-actor address, so there is no
    /// actor-specific hostname left to hand back here. A caller of `GetSandboxDomain` still
    /// needs the routing header to reach the right actor through this endpoint.
    pub fn actor_endpoint(&self, _actor: &str, _atespace: &str) -> String {
        self.router_endpoint.to_string()
    }
}
