#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        acyclic_modules,
        reason = "the adapter modules implement one provider across many files and reach each other through its impl blocks"
    )
)]
#![recursion_limit = "256"]

mod checkpoint;
mod file_ops;
mod fresh_id;
mod hardening_overrides;
mod launch_guard;
mod port_forward;
mod profile;
#[cfg(test)]
mod support_tests;

pub mod adapter;
pub mod browser;
pub mod digest_cache;
pub mod heartbeat;
pub mod nats_lifecycle;
pub mod owner_store;
pub mod registry_auth;
pub mod rollback_state;
pub mod snapshot;
pub mod terminal;
pub mod warm_pool;
pub mod worker;

pub use checkpoint::{
    CHECKPOINT_LAYER_ANNOTATION, CHECKPOINT_NAME_ANNOTATION, checkpoint_layer, checkpoint_name_from_annotations,
    checkpoint_storage_dir, gzip_checkpoint, is_checkpoint_layer, tar_checkpoint_dir, untar_checkpoint,
};

pub use profile::{PROFILE, profile_with_resources, resources};
pub use warm_pool::{WarmContainer, WarmPool, WarmPoolConfig};
