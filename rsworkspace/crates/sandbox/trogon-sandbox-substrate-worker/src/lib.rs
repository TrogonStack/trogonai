//! Agent Substrate provider for the sandbox stack.
//!
//! Substrate exposes sandboxes as `Actor`s driven through the `ateapi.Control` gRPC service.
//! There is no exec, attach or port-forward RPC anywhere in that surface, so every command
//! this worker runs travels over HTTP to an agent inside the actor, reached through the
//! substrate router's L7 ingress. That single constraint shapes most of this crate.

mod actor_index;
mod adapter;
mod agent;
mod ate_client;
mod config;
mod naming;
mod nats_lifecycle;
mod profile;
pub mod proto;
mod recovery;
mod resources;
mod router_tls;
mod template;
mod worker;
mod workspace;

#[cfg(test)]
mod fleet_tests;

pub use actor_index::{
    ACTOR_KV_BUCKET, ActorClaimId, ActorClaimIdError, ActorEntry, ActorIndex, ActorIndexError, ActorIndexResult,
    ActorOwner, ActorUid, ActorUidError, NatsActorIndex, claim, entry_key,
};
pub use adapter::{ActiveLease, PurgeOutcome, SubstrateError, SubstrateProvider, supports_tier};
pub use agent::{ActorAgent, ActorTarget, AgentError, ChunkSink, DiscardChunks};
pub use ate_client::{
    AteError, AteResult, BearerTokenFile, ConnectError, ConnectSecurity, Control, ResourceRevision, TlsTrust,
    TonicControl,
};
pub use config::{
    DEFAULT_ROUTER_ENDPOINT, RouterEndpoint, RouterEndpointError, SANDBOX_CLASS_LABEL, SIZE_CLASS_LABEL, SandboxClass,
    SandboxClassError, SubstrateConfig, SubstrateConfigError,
};
pub use naming::{ActorName, Atespace, NamingError, is_managed};
pub use nats_lifecycle::NatsLifecycleEmitter;
pub use profile::PROFILE;
pub use recovery::{REAP_INTERVAL, RecoveryError, RecoveryReport, lease_id_of, reap, reap_loop, recover};
pub use resources::{ActorResources, ActorResourcesError, ImageDigestError, validate_image_digest};
pub use router_tls::{RouterTlsConfig, RouterTlsError};
pub use template::{
    NatsTemplateIndex, TEMPLATE_KV_BUCKET, TemplateError, TemplateIndex, TemplateIndexError, TemplateKey,
    TemplatePhase, TemplateRecord, TemplateRegistry,
};
pub use worker::{ServeError, WorkerArgs, WorkerError, run, run_with_authority, serve, spawn_heartbeat_task};
pub use workspace::{WORKSPACE_DIR, WorkspaceError, materialize};
