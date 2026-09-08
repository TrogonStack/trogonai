//! Publishing a decider component into the module store.
//!
//! [ADR#0058](../../../docs/adr/0058-decider-module-distribution.md) puts the
//! conformance gate here rather than at host start: a component enters the
//! store only after its suite passes, so every host that fetches it is fetching
//! something already known to be conformant. A host that re-ran the suite would
//! be paying for it on every replica, on every restart, to learn something the
//! publisher already established, and would still have no answer for what to do
//! about a module that fails it at three in the morning.
//!
//! The reference a component is published under is read out of the component's
//! own descriptor, never taken from the command line. A publisher that accepted
//! both would be the place where the key and the descriptor first disagree, and
//! [`DeciderHost::start`](trogon_decider_nats_server::DeciderHost::start)
//! refuses exactly that disagreement.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use async_nats::jetstream;
use trogon_decider_nats_server::{
    ModuleReference, ObjectStoreModuleSource, ProvisionModuleBucketError, PublishModuleError,
};
use trogon_decider_test::Suite;
use trogon_decider_test::conformance::{OutputFormat, Strictness, run_suite};
use trogon_decider_wasm_runtime::{LoadWasmDeciderError, WasmDeciderEngine, WasmDeciderModule, WasmEngineError};

/// One component, its suite, and where it is going.
pub struct PublishRequest<'a> {
    /// The compiled component's bytes.
    pub component: &'a [u8],
    /// The suite that must pass before the component enters the store.
    pub suite: &'a Suite,
    /// Object store bucket to publish into.
    pub bucket: &'a str,
    /// How the suite run reports itself.
    pub format: OutputFormat,
}

/// Runs the gate and, only if it passes, publishes the component.
///
/// Returns the reference the component was published under, which is the
/// reference an operator then configures a host with.
pub async fn publish(js: &jetstream::Context, request: &PublishRequest<'_>) -> Result<ModuleReference, PublishError> {
    // Strict is not a parameter: a component with a declared command no
    // scenario exercises is a component whose store entry claims coverage it
    // does not have, and the store entry is what every host trusts afterwards.
    run_suite(request.component, request.suite, request.format, Strictness::Strict)
        .map_err(|source| PublishError::Suite { source })?;

    // The same load the host will perform, so a component that passes its suite
    // under the sim but cannot be instantiated against an empty linker fails
    // here rather than at some replica's next restart.
    let engine = WasmDeciderEngine::new(trogon_decider_wasm_runtime::WasmEngineConfig::default())?;
    let module = WasmDeciderModule::load(engine, request.component)?;
    let reference = ModuleReference::new(module.name().clone(), module.version().clone());

    ObjectStoreModuleSource::provision(js, request.bucket)
        .await?
        .publish(&reference, request.component)
        .await?;

    Ok(reference)
}

/// Why a component did not reach the store.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The conformance suite did not pass, so nothing was published.
    #[error("conformance suite failed; nothing was published: {source}")]
    Suite {
        #[source]
        source: anyhow::Error,
    },
    /// The wasm engine the load probe needs could not be built.
    #[error("{0}")]
    Engine(#[from] WasmEngineError),
    /// The component passed its suite but is not one a host could load.
    #[error("component is not loadable as a decider module; nothing was published: {0}")]
    Load(#[from] LoadWasmDeciderError),
    /// The module bucket could not be reached.
    #[error("{0}")]
    Bucket(#[from] ProvisionModuleBucketError),
    /// The bucket was reachable but the write did not land.
    #[error("{0}")]
    Write(#[from] PublishModuleError),
}
