//! Serving the ADR#0057 binding.
//!
//! Startup and request handling are separate on purpose. Everything that can
//! be wrong about a deployment (a module that will not load, two modules
//! claiming one command type, an events stream that does not capture a
//! module's subtree) is resolved in [`DeciderHost::start`] and refuses to
//! produce a host. What remains at request time is only what a caller can get
//! wrong, and every one of those answers with a `CommandOutcome` rather than
//! with silence.

use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;

use async_nats::{Client, HeaderMap, Message, SubscribeError, jetstream};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use trogon_decider_nats::{
    DuplicateWindowExceedsMaxAgeError, EnsureBucketError, EnsureStreamError, JetStreamStore, apply_duplicate_window,
    ensure_bucket, ensure_stream,
};
use trogon_decider_runtime::{
    ConcurrencyAdmission, DrainableSnapshotTaskScheduler, ReplayLimit, SnapshotTaskScheduler as _,
};
use trogon_decider_wasm_runtime::{
    DeciderRegistry, DeciderRegistryHandle, LoadWasmDeciderError, ModuleName, RegisterModuleError,
    WasmCommandExecution, WasmDeciderEngine, WasmDeciderModule, WasmEngineConfig, WasmEngineError,
};

use crate::command_subject::CommandSubjects;
use crate::config::ServerConfig;
use crate::constants::{CONTENT_TYPE_HEADER, PROTOBUF_CONTENT_TYPE, TROGON_DECIDER_OUTCOME_HEADER};
use crate::event_subject::ModuleEventSubjects;
use crate::module_reference::ModuleReference;
use crate::module_source::ModuleSource;
use crate::outcome::CommandReply;
use crate::request::CommandRequest;

/// Resolves one inbound message onto the module that owns it.
///
/// Separate from the execution side so every way a request can fail to route
/// is exercisable without a NATS server or a loaded module.
pub struct CommandRouter {
    subjects: CommandSubjects,
    registry: Arc<DeciderRegistryHandle>,
}

impl CommandRouter {
    /// Binds a router to the subject projection and the routable set.
    pub const fn new(subjects: CommandSubjects, registry: Arc<DeciderRegistryHandle>) -> Self {
        Self { subjects, registry }
    }

    /// The routable set this router reads. Shared, so a later activation is visible here.
    pub fn registry(&self) -> &Arc<DeciderRegistryHandle> {
        &self.registry
    }

    /// The subject projection this router is bound to.
    pub const fn subjects(&self) -> &CommandSubjects {
        &self.subjects
    }

    /// Resolves a delivery into the module and inputs one execution needs.
    ///
    /// The envelope is parsed before the route is resolved: a caller that sent
    /// an unparseable header learns that, rather than learning "unroutable"
    /// and going to look at a deployment that is fine.
    pub fn route(
        &self,
        subject: &str,
        payload: Vec<u8>,
        headers: Option<&HeaderMap>,
    ) -> Result<RoutedCommand, CommandReply> {
        let command_type = self
            .subjects
            .command_type_for(subject)
            .map_err(|error| CommandReply::invalid_request(&error))?;
        let request = CommandRequest::parse(&command_type, payload, headers)
            .map_err(|error| CommandReply::invalid_request(&error))?;
        let module = self
            .registry
            .route(&command_type)
            .map_err(|error| CommandReply::unroutable(&error))?;

        Ok(RoutedCommand { module, request })
    }
}

/// One delivery, resolved onto the module that will execute it.
///
/// Holds the resolved `Arc` for the rest of the dispatch, so a concurrent
/// activation cannot move this command to a different module partway through.
pub struct RoutedCommand {
    module: Arc<WasmDeciderModule>,
    request: CommandRequest,
}

impl std::fmt::Debug for RoutedCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutedCommand")
            .field("module", self.module.name())
            .field("command_type", &self.request.command().type_)
            .finish_non_exhaustive()
    }
}

impl RoutedCommand {
    /// The module this command resolved to.
    pub fn module(&self) -> &WasmDeciderModule {
        &self.module
    }

    /// The command as the transport delivered it.
    pub const fn request(&self) -> &CommandRequest {
        &self.request
    }
}

/// A running host: the routable set, one store per module, and the limits
/// every execution runs under.
pub struct DeciderHost {
    router: CommandRouter,
    stores: HashMap<ModuleName, JetStreamStore<ModuleEventSubjects>>,
    snapshots: DrainableSnapshotTaskScheduler,
    admission: ConcurrencyAdmission,
    replay_limit: Option<ReplayLimit>,
}

impl DeciderHost {
    /// Fetches every configured module, provisions its storage, and assembles the host.
    ///
    /// Provisioning happens once, here, against the union of every module's
    /// event subtree. A module added to the configuration later widens that
    /// union, and [`ensure_stream`] refuses an existing stream that does not
    /// already cover it, so the mismatch surfaces at startup rather than as a
    /// command whose events land nowhere.
    pub async fn start<S: ModuleSource>(
        config: &ServerConfig,
        modules: &S,
        js: jetstream::Context,
    ) -> Result<Self, StartupError> {
        let engine = WasmDeciderEngine::new(WasmEngineConfig::default())?;
        let modules = load_modules(&engine, modules, &config.modules).await?;
        let module_names = modules.iter().map(|module| module.name().clone()).collect::<Vec<_>>();

        let events_stream = ensure_stream(&js, events_stream_config(config, &module_names)?).await?;
        let snapshot_bucket = ensure_bucket(&js, snapshot_bucket_config(config)).await?;
        let store_builder = JetStreamStore::builder(js, events_stream, snapshot_bucket);

        let stores = module_names
            .iter()
            .map(|name| {
                let store = store_builder
                    .clone()
                    .with_subject_resolver(ModuleEventSubjects::new(name));
                (name.clone(), store)
            })
            .collect();

        let mut registry = DeciderRegistry::builder();
        for module in modules {
            registry = registry.register(module)?;
        }

        Ok(Self {
            router: CommandRouter::new(config.subjects.clone(), Arc::new(registry.build_handle())),
            stores,
            snapshots: DrainableSnapshotTaskScheduler::new(),
            admission: ConcurrencyAdmission::new(config.admission_limit),
            replay_limit: config.replay_limit,
        })
    }

    /// The router this host dispatches through.
    pub const fn router(&self) -> &CommandRouter {
        &self.router
    }

    /// Runs one already-routed command to its outcome.
    pub async fn execute(&self, routed: RoutedCommand) -> CommandReply {
        let Some(store) = self.stores.get(routed.module.name()) else {
            // Unreachable while the routable set is fixed at startup, and the
            // reason this is a fault rather than a panic: the day a control
            // plane can activate a module at runtime, this is the request that
            // finds out its storage was never provisioned.
            let error = UnprovisionedModuleError {
                module_name: routed.module.name().clone(),
            };
            error!(module = %error.module_name, "routed a command to a module with no store");
            return CommandReply::internal(&error);
        };

        let execution = WasmCommandExecution::new(&routed.module, store, routed.request.command())
            .with_snapshot_store(store, &self.snapshots)
            .with_expected_revision(routed.request.expected_revision())
            .with_replay_limit(self.replay_limit)
            .with_command_id(routed.request.command_id())
            .with_admission(&self.admission);

        match execution.execute().await {
            Ok(result) => CommandReply::decided(&result),
            Err(error) => CommandReply::from_command_error(routed.module.name(), &error),
        }
    }

    /// Waits for every snapshot write this host scheduled to finish.
    pub async fn drain(&self) {
        self.snapshots.drain().await;
    }
}

/// Subscribes on the host's subject subtree and answers until a shutdown signal.
pub async fn serve(host: Arc<DeciderHost>, client: Client, config: &ServerConfig) -> Result<(), ServeError> {
    let shutdown = CancellationToken::new();
    let shutdown_for_signal = shutdown.clone();
    drop(tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        shutdown_for_signal.cancel();
    }));

    serve_until(host, client, config, shutdown).await
}

/// Subscribes on the host's subject subtree and answers until `shutdown` is cancelled.
///
/// Each delivery is handled on its own task so the admission limit, not the
/// receive loop, is what bounds concurrency: a serialized loop would cap the
/// host at one in-flight command and make the limiter unobservable.
pub async fn serve_until(
    host: Arc<DeciderHost>,
    client: Client,
    config: &ServerConfig,
    shutdown: CancellationToken,
) -> Result<(), ServeError> {
    let pattern = config.subjects.subscription_pattern();
    let mut ingress = client
        .queue_subscribe(pattern.clone(), config.queue_group.clone())
        .await
        .map_err(|source| ServeError::Subscribe {
            pattern: pattern.clone(),
            source,
        })?;

    info!(
        subject = %pattern,
        queue_group = %config.queue_group,
        routes = host.router.registry().routes().len(),
        "decider host subscribed",
    );

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                info!("decider host shutdown signal received");
                break;
            }
            incoming = ingress.next() => {
                let Some(message) = incoming else {
                    warn!("decider host ingress subscription closed");
                    break;
                };
                let host = Arc::clone(&host);
                let client = client.clone();
                drop(tokio::spawn(async move { handle(&host, &client, message).await }));
            }
        }
    }

    drop(ingress);
    host.drain().await;
    info!("decider host shutdown complete");
    Ok(())
}

async fn handle(host: &DeciderHost, client: &Client, message: Message) {
    let Message {
        subject,
        payload,
        reply,
        headers,
        ..
    } = message;

    let Some(reply_subject) = reply else {
        // ADR#0057 binds commands to core request/reply only. Executing one
        // nobody is waiting on would append events whose outcome no caller
        // ever learns, which is the fire-and-forget submission path the ADR
        // rules out rather than a degraded version of this one.
        warn!(%subject, "dropped a command sent with no reply subject");
        return;
    };

    let reply = match host.router.route(subject.as_str(), payload.to_vec(), headers.as_ref()) {
        Ok(routed) => host.execute(routed).await,
        Err(reply) => reply,
    };

    let mut reply_headers = HeaderMap::new();
    reply_headers.insert(CONTENT_TYPE_HEADER, PROTOBUF_CONTENT_TYPE);
    reply_headers.insert(TROGON_DECIDER_OUTCOME_HEADER, reply.header_value());

    if let Err(error) = client
        .publish_with_headers(reply_subject, reply_headers, reply.encode().into())
        .await
    {
        error!(%subject, outcome = reply.header_value(), %error, "failed to publish a command outcome");
    }
}

/// Fetches every configured reference and checks that what came back is what
/// was asked for, per ADR#0058.
///
/// The identity check is against the component's own descriptor rather than
/// against the key it was stored under, because the key is a claim the
/// publisher made and the descriptor is what the guest will actually behave
/// as. A store that has the two disagree is a store that would otherwise serve
/// one module's commands under another module's name.
async fn load_modules<S: ModuleSource>(
    engine: &WasmDeciderEngine,
    source: &S,
    references: &[ModuleReference],
) -> Result<Vec<WasmDeciderModule>, StartupError> {
    let mut modules = Vec::with_capacity(references.len());
    for reference in references {
        let bytes = source
            .fetch(reference)
            .await
            .map_err(|error| StartupError::FetchModule {
                reference: reference.clone(),
                store: source.describe(),
                source: Box::new(error),
            })?;
        let module = WasmDeciderModule::load(engine.clone(), &bytes).map_err(|error| StartupError::LoadModule {
            reference: reference.clone(),
            source: error,
        })?;
        let declared = ModuleReference::new(module.name().clone(), module.version().clone());
        if &declared != reference {
            return Err(StartupError::ModuleIdentityMismatch {
                reference: reference.clone(),
                declared,
            });
        }
        modules.push(module);
    }
    Ok(modules)
}

fn events_stream_config(
    config: &ServerConfig,
    module_names: &[ModuleName],
) -> Result<jetstream::stream::Config, StartupError> {
    let stream = jetstream::stream::Config {
        name: config.events_stream.clone(),
        subjects: module_names
            .iter()
            .map(|name| ModuleEventSubjects::new(name).subscription_pattern())
            .collect(),
        // The append path publishes every command's events as one atomic
        // batch, so a stream without it cannot store a multi-event decision.
        allow_atomic_publish: true,
        ..Default::default()
    };

    Ok(apply_duplicate_window(stream, config.duplicate_window)?)
}

fn snapshot_bucket_config(config: &ServerConfig) -> jetstream::kv::Config {
    jetstream::kv::Config {
        bucket: config.snapshot_bucket.clone(),
        // A snapshot is a cache of a stream's fold, so only its latest
        // revision is ever read and older ones are pure storage cost.
        history: 1,
        ..Default::default()
    }
}

/// Why the host could not be assembled from its configuration.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// The shared wasmtime engine could not be built.
    #[error("failed to build the wasm engine: {0}")]
    Engine(#[from] WasmEngineError),
    /// The module store could not produce a configured reference's bytes.
    #[error("failed to fetch module '{reference}' from {store}: {source}")]
    FetchModule {
        reference: ModuleReference,
        store: String,
        #[source]
        source: Box<dyn StdError + Send + Sync>,
    },
    /// A fetched component is not a loadable decider component.
    #[error("failed to load module '{reference}': {source}")]
    LoadModule {
        reference: ModuleReference,
        #[source]
        source: LoadWasmDeciderError,
    },
    /// A fetched component is not the module that was requested.
    #[error("module '{reference}' resolved to a component declaring '{declared}'")]
    ModuleIdentityMismatch {
        reference: ModuleReference,
        declared: ModuleReference,
    },
    /// Two configured modules claim the same command type.
    #[error("{0}")]
    Register(#[from] RegisterModuleError),
    /// The requested duplicate window is wider than the stream's max age.
    #[error("{0}")]
    DuplicateWindow(#[from] DuplicateWindowExceedsMaxAgeError),
    /// The events stream could not be created or does not cover every module.
    #[error("{0}")]
    EventsStream(#[from] EnsureStreamError),
    /// The snapshot bucket could not be created or diverges from what is required.
    #[error("{0}")]
    SnapshotBucket(#[from] EnsureBucketError),
}

/// Why the host stopped serving.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The host could not take its subscription.
    #[error("failed to subscribe on '{pattern}': {source}")]
    Subscribe {
        pattern: String,
        #[source]
        source: SubscribeError,
    },
}

/// A command routed to a module the host never provisioned storage for.
#[derive(Debug, thiserror::Error)]
#[error("module '{module_name}' is routable but has no provisioned store")]
struct UnprovisionedModuleError {
    module_name: ModuleName,
}

#[cfg(test)]
mod tests;
