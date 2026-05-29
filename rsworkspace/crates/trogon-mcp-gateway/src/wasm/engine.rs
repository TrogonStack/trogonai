//! Process-wide Wasmtime engine and digest-scoped bundle handles.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use wasmtime::Engine;
use wasmtime::component::{Component, Linker};

use crate::bundle::{LoadedBundle, HOST_TARGET_WIT};
use crate::cel_builtins::HostEvalContext;

use super::bindings::{contract_identity, RequestCtx, WIT_PACKAGE, WIT_VERSION};
use super::config::PoolConfig;
use super::error::{EvaluateOutcome, WasmEngineError};
use super::pool::ComponentPool;
use super::store_state::WasmStoreState;
use super::wasi_stub;

/// Process-wide Wasmtime engine with digest-keyed component pools.
pub struct WasmEngine {
    engine: Engine,
    config: PoolConfig,
    handles: RwLock<HashMap<BundleVersionKey, Arc<LoadedPools>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BundleVersionKey {
    tenant_id: String,
    bundle_name: String,
    manifest_digest: String,
}

struct LoadedPools {
    pools: HashMap<String, Arc<ComponentPool>>,
}

/// Handle to warmed pools for one loaded bundle revision.
#[derive(Clone)]
pub struct WasmBundleHandle {
    key: BundleVersionKey,
    target_wit: String,
    default_component: Option<String>,
    pools: HashMap<String, Arc<ComponentPool>>,
}

impl WasmEngine {
    /// Creates the shared engine configured for component model + fuel.
    pub fn new(config: PoolConfig) -> Result<Self, WasmEngineError> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.wasm_component_model(true);
        wasm_config.concurrency_support(true);
        wasm_config.consume_fuel(true);
        wasm_config.max_wasm_stack(512 * 1024);
        let engine = Engine::new(&wasm_config)
            .map_err(|err| WasmEngineError::Config(err.to_string()))?;
        Ok(Self {
            engine,
            config,
            handles: RwLock::new(HashMap::new()),
        })
    }

    #[must_use]
    pub fn config(&self) -> PoolConfig {
        self.config
    }

    /// Compiles bundle components and prewarms digest-scoped pools.
    pub async fn load(&self, bundle: &LoadedBundle) -> Result<WasmBundleHandle, WasmEngineError> {
        let identity = contract_identity();
        let expected = format!("{}@{}", identity.package, identity.version);
        if bundle.manifest.target_wit != expected && bundle.manifest.target_wit != HOST_TARGET_WIT {
            return Err(WasmEngineError::TargetWit {
                expected: expected.clone(),
                got: bundle.manifest.target_wit.clone(),
            });
        }
        if bundle.components.is_empty() {
            return Err(WasmEngineError::NoComponents);
        }

        let scope = bundle.scope.clone();
        let key = BundleVersionKey {
            tenant_id: scope.tenant,
            bundle_name: bundle.manifest.name.clone(),
            manifest_digest: bundle.manifest_digest.as_hex().to_string(),
        };

        let linker: Linker<WasmStoreState> = Linker::new(&self.engine);

        let mut pools = HashMap::new();
        for component in &bundle.components {
            let compiled = Component::from_binary(&self.engine, &component.bytes).map_err(|err| {
                WasmEngineError::Compile {
                    component_id: component.entry.id.clone(),
                    detail: err.to_string(),
                }
            })?;
            let mut component_linker = linker.clone();
            wasi_stub::prepare_linker(&mut component_linker, &compiled).map_err(|err| {
                WasmEngineError::Link {
                    component_id: component.entry.id.clone(),
                    detail: err.to_string(),
                }
            })?;
            let pool = ComponentPool::new(
                self.engine.clone(),
                component_linker,
                compiled,
                self.config,
                Arc::from(component.entry.id.as_str()),
            )?;
            pools.insert(component.entry.id.clone(), pool);
        }

        let default_component = bundle.components.first().map(|c| c.entry.id.clone());
        let loaded = Arc::new(LoadedPools { pools: pools.clone() });
        self.handles.write().await.insert(key.clone(), loaded);

        Ok(WasmBundleHandle {
            key,
            target_wit: bundle.manifest.target_wit.clone(),
            default_component,
            pools,
        })
    }

    /// Runs `policy-guest.evaluate` on a pooled instance for the given component id.
    pub async fn evaluate(
        &self,
        handle: &WasmBundleHandle,
        component_id: Option<&str>,
        request: &RequestCtx,
        host: HostEvalContext,
    ) -> Result<EvaluateOutcome, WasmEngineError> {
        let component_id = component_id
            .or(handle.default_component.as_deref())
            .ok_or_else(|| WasmEngineError::UnknownComponent {
                component_id: String::new(),
            })?;
        let pool = handle.pools.get(component_id).ok_or_else(|| WasmEngineError::UnknownComponent {
            component_id: component_id.to_string(),
        })?;
        pool.evaluate(request, host).await
    }

    /// Drains and removes pools for a bundle revision (hot-swap eviction).
    pub async fn drop_handle(&self, handle: WasmBundleHandle) {
        if let Some(loaded) = self.handles.write().await.remove(&handle.key) {
            for pool in loaded.pools.values() {
                pool.drain().await;
            }
        }
        for pool in handle.pools.values() {
            pool.drain().await;
        }
    }

    #[must_use]
    pub fn contract_pins(&self) -> (&'static str, &'static str) {
        (WIT_PACKAGE, WIT_VERSION)
    }

    #[cfg(test)]
    pub(crate) async fn register_handle_for_test(&self, handle: WasmBundleHandle) {
        self.handles
            .write()
            .await
            .insert(handle.key.clone(), Arc::new(LoadedPools {
                pools: handle.pools.clone(),
            }));
    }

    #[cfg(test)]
    pub async fn registered_digest_count(&self) -> usize {
        self.handles.read().await.len()
    }
}

impl WasmBundleHandle {
    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        &self.key.manifest_digest
    }

    #[must_use]
    pub fn component_ids(&self) -> impl Iterator<Item = &String> {
        self.pools.keys()
    }

    #[must_use]
    pub fn target_wit(&self) -> &str {
        &self.target_wit
    }
}

#[cfg(test)]
mod tests {
    use crate::bundle::{
        BundleManifest, BundleScope, ComponentEntry, LoadedBundle, LoadedComponent, ManifestDigest,
        MANIFEST_FILENAME, Signing, HOST_TARGET_WIT,
    };

    use super::*;

    fn fixture_bundle_with_invalid_wasm() -> LoadedBundle {
        LoadedBundle {
            manifest: BundleManifest {
                name: "acme/demo".into(),
                version: "1.0.0".into(),
                target_wit: HOST_TARGET_WIT.into(),
                min_gateway_version: "0.0.1".into(),
                cel_version: None,
                author: "platform".into(),
                created_at: "2026-05-28T00:00:00Z".into(),
                description: "demo".into(),
                capabilities: Default::default(),
                signing: Signing {
                    nkey_pub: "UABTRUSTED".into(),
                },
                programs: vec![],
                components: vec![ComponentEntry {
                    id: "policy".into(),
                    path: "components/policy.wasm".into(),
                    sha256: "00".into(),
                    mode: None,
                }],
                schemas: vec![],
            },
            manifest_filename: MANIFEST_FILENAME.into(),
            manifest_bytes: br#"name = "acme/demo""#.to_vec(),
            manifest_digest: ManifestDigest::from_bytes(b"manifest"),
            scope: BundleScope {
                tenant: "acme".into(),
                slug: "demo".into(),
            },
            signer_nkey: "UABTRUSTED".into(),
            programs: vec![],
            components: vec![LoadedComponent {
                entry: ComponentEntry {
                    id: "policy".into(),
                    path: "components/policy.wasm".into(),
                    sha256: "00".into(),
                    mode: None,
                },
                bytes: vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
            }],
            schemas: vec![],
        }
    }

    #[test]
    fn engine_cold_start_configures_component_model() {
        let engine = WasmEngine::new(PoolConfig::for_tests()).expect("engine");
        let (package, version) = engine.contract_pins();
        assert_eq!(package, WIT_PACKAGE);
        assert_eq!(version, WIT_VERSION);
    }

    #[tokio::test]
    async fn load_rejects_invalid_wasm_bytes() {
        let engine = WasmEngine::new(PoolConfig::for_tests()).expect("engine");
        let bundle = fixture_bundle_with_invalid_wasm();
        let err = match engine.load(&bundle).await {
            Err(err) => err,
            Ok(_) => panic!("expected invalid wasm compile failure"),
        };
        assert!(matches!(err, WasmEngineError::Compile { .. }));
    }

    #[tokio::test]
    async fn drop_handle_is_idempotent_for_empty_pools() {
        let engine = WasmEngine::new(PoolConfig::for_tests()).expect("engine");
        let handle = WasmBundleHandle {
            key: BundleVersionKey {
                tenant_id: "acme".into(),
                bundle_name: "demo".into(),
                manifest_digest: "abc".into(),
            },
            target_wit: HOST_TARGET_WIT.into(),
            default_component: None,
            pools: HashMap::new(),
        };
        engine.drop_handle(handle).await;
    }

    fn shell_pool_for_engine(config: PoolConfig) -> Arc<ComponentPool> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.wasm_component_model(true);
        let component_engine = Engine::new(&wasm_config).expect("engine");
        const MINIMAL: &[u8] = include_bytes!("testdata/minimal.component.wasm");
        let component = Component::from_binary(&component_engine, MINIMAL)
            .expect("minimal component");
        let linker = Linker::new(&component_engine);
        ComponentPool::new_without_prewarm(
            component_engine,
            linker,
            component,
            config,
            Arc::from("policy"),
        )
    }

    #[tokio::test]
    async fn drop_handle_drains_old_revision_pools() {
        let engine = WasmEngine::new(PoolConfig::for_tests()).expect("engine");
        let pool_v1 = shell_pool_for_engine(engine.config());
        let pool_v2 = shell_pool_for_engine(engine.config());

        let handle_v1 = WasmBundleHandle {
            key: BundleVersionKey {
                tenant_id: "acme".into(),
                bundle_name: "demo".into(),
                manifest_digest: "digest-v1".into(),
            },
            target_wit: HOST_TARGET_WIT.into(),
            default_component: Some("policy".into()),
            pools: HashMap::from([("policy".into(), Arc::clone(&pool_v1))]),
        };
        let handle_v2 = WasmBundleHandle {
            key: BundleVersionKey {
                tenant_id: "acme".into(),
                bundle_name: "demo".into(),
                manifest_digest: "digest-v2".into(),
            },
            target_wit: HOST_TARGET_WIT.into(),
            default_component: Some("policy".into()),
            pools: HashMap::from([("policy".into(), pool_v2)]),
        };

        engine.register_handle_for_test(handle_v1.clone()).await;
        engine.register_handle_for_test(handle_v2).await;
        assert_eq!(engine.registered_digest_count().await, 2);

        engine.drop_handle(handle_v1).await;
        assert_eq!(engine.registered_digest_count().await, 1);
        assert!(pool_v1.is_shutting_down());
    }
}
