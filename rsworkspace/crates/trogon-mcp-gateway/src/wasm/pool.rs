//! Component instance pool keyed by bundle digest and component id (ADR 0025).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Semaphore;
use wasmtime::Engine;
use wasmtime::component::{Component, Linker};

use crate::cel_builtins::HostEvalContext;

use super::bindings::{PolicyDecision, RequestCtx};
use super::config::PoolConfig;
use super::error::{EvaluateOutcome, WasmEngineError, WasmFaultCode};
use super::runtime::PolicyBundle;
use super::runtime::trogon::mcp_policy::policy_types::{
    PolicyDecision as WitPolicyDecision, RequestCtx as WitRequestCtx, SpanContext as WitSpanContext,
    ToolDescriptor as WitToolDescriptor,
};
use super::store_state::WasmStoreState;

/// Identity for a digest-scoped component pool.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub tenant_id: String,
    pub bundle_name: String,
    pub manifest_digest: String,
    pub component_id: String,
    pub target_wit: String,
}

struct PooledInstance {
    guest: PolicyBundle,
    store: wasmtime::Store<WasmStoreState>,
}

struct PoolInner {
    engine: Engine,
    linker: Linker<WasmStoreState>,
    component: Component,
    config: PoolConfig,
    component_id: Arc<str>,
    idle: tokio::sync::Mutex<Vec<PooledInstance>>,
    live: AtomicU64,
    next_instance_id: AtomicU64,
    shutting_down: AtomicBool,
}

/// Checkout/return pool for one compiled component at one manifest digest.
pub struct ComponentPool {
    inner: Arc<PoolInner>,
    checkout: Semaphore,
}

impl ComponentPool {
    pub(crate) fn new(
        engine: Engine,
        linker: Linker<WasmStoreState>,
        component: Component,
        config: PoolConfig,
        component_id: Arc<str>,
    ) -> Result<Arc<Self>, WasmEngineError> {
        let max = config.max_instances.max(1) as usize;
        let pool = Arc::new(Self {
            inner: Arc::new(PoolInner {
                engine,
                linker,
                component,
                config,
                component_id,
                idle: tokio::sync::Mutex::new(Vec::new()),
                live: AtomicU64::new(0),
                next_instance_id: AtomicU64::new(1),
                shutting_down: AtomicBool::new(false),
            }),
            checkout: Semaphore::new(max),
        });
        pool.prewarm()?;
        Ok(pool)
    }

    fn prewarm(&self) -> Result<(), WasmEngineError> {
        let count = self.inner.config.prewarm;
        let mut created = 0_u32;
        for _ in 0..count {
            match self.create_instance(HostEvalContext::for_tests()) {
                Ok(instance) => {
                    self.inner.idle.blocking_lock().push(instance);
                    created += 1;
                }
                Err(err) => {
                    if created == 0 {
                        return Err(err);
                    }
                    break;
                }
            }
        }
        if created == 0 {
            return Err(WasmEngineError::Init {
                component_id: self.inner.component_id.to_string(),
                detail: "prewarm produced zero healthy instances".into(),
            });
        }
        Ok(())
    }

    fn create_instance(
        &self,
        host: HostEvalContext,
    ) -> Result<PooledInstance, WasmEngineError> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(WasmEngineError::PoolShutDown);
        }
        let live = self.inner.live.fetch_add(1, Ordering::AcqRel) + 1;
        if live > u64::from(self.inner.config.max_instances) {
            self.inner.live.fetch_sub(1, Ordering::AcqRel);
            return Err(WasmEngineError::Internal("pool at capacity".into()));
        }

        let instance_id = self.inner.next_instance_id.fetch_add(1, Ordering::Relaxed);
        let state = WasmStoreState::new(
            host,
            self.inner.config,
            Arc::clone(&self.inner.component_id),
            instance_id,
        );
        let mut store = wasmtime::Store::new(&self.inner.engine, state);
        store
            .set_fuel(self.inner.config.fuel_init)
            .map_err(|err| WasmEngineError::Internal(err.to_string()))?;

        let guest = PolicyBundle::instantiate(&mut store, &self.inner.component, &self.inner.linker)
            .map_err(|err| {
                self.inner.live.fetch_sub(1, Ordering::AcqRel);
                WasmEngineError::Link {
                    component_id: self.inner.component_id.to_string(),
                    detail: err.to_string(),
                }
            })?;

        guest
            .trogon_mcp_policy_policy_guest()
            .call_init(&mut store)
            .map_err(|err| {
                self.inner.live.fetch_sub(1, Ordering::AcqRel);
                WasmEngineError::Init {
                    component_id: self.inner.component_id.to_string(),
                    detail: err.to_string(),
                }
            })?
            .map_err(|failure| {
                self.inner.live.fetch_sub(1, Ordering::AcqRel);
                WasmEngineError::Init {
                    component_id: self.inner.component_id.to_string(),
                    detail: format!("{}: {}", failure.code, failure.message),
                }
            })?;

        Ok(PooledInstance { guest, store })
    }

    /// Borrows an instance, runs `evaluate`, and returns it unless poisoned.
    pub async fn evaluate(
        self: &Arc<Self>,
        request: &RequestCtx,
        host: HostEvalContext,
    ) -> Result<EvaluateOutcome, WasmEngineError> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(WasmEngineError::PoolShutDown);
        }

        let _permit = tokio::time::timeout(
            Duration::from_millis(self.inner.config.acquire_timeout_ms),
            self.checkout.acquire(),
        )
        .await
        .map_err(|_| WasmEngineError::Internal("pool acquire timeout".into()))?
        .map_err(|_| WasmEngineError::Internal("pool semaphore closed".into()))?;

        let mut instance = self.checkout_instance(host).await?;
        let wit_input = to_wit_request_ctx(request);
        store_reset_fuel(&mut instance.store, self.inner.config.fuel_evaluate)?;

        let eval_result = instance
            .guest
            .trogon_mcp_policy_policy_guest()
            .call_evaluate(&mut instance.store, &wit_input);
        let poisoned = eval_result.is_err();
        let outcome = match eval_result {
            Ok(decision) => EvaluateOutcome::Decision(from_wit_decision(decision)),
            Err(err) => EvaluateOutcome::Fault {
                code: classify_trap(&err),
                message: err.to_string(),
            },
        };

        if poisoned {
            self.inner.live.fetch_sub(1, Ordering::AcqRel);
        } else {
            instance.store.data_mut().reset_import_calls();
            self.inner.idle.lock().await.push(instance);
        }
        Ok(outcome)
    }

    async fn checkout_instance(
        &self,
        host: HostEvalContext,
    ) -> Result<PooledInstance, WasmEngineError> {
        if let Some(mut instance) = self.inner.idle.lock().await.pop() {
            instance.store.data_mut().host = host;
            instance.store.data_mut().reset_import_calls();
            return Ok(instance);
        }
        self.create_instance(host)
    }

    /// Stops new checkouts and drops idle instances; in-flight work continues until return.
    pub async fn drain(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        self.inner.idle.lock().await.clear();
    }

    #[must_use]
    pub fn idle_count(&self) -> usize {
        self.inner.idle.try_lock().map_or(0, |idle| idle.len())
    }

    #[must_use]
    pub fn live_count(&self) -> u64 {
        self.inner.live.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn new_without_prewarm(
        engine: Engine,
        linker: Linker<WasmStoreState>,
        component: Component,
        config: PoolConfig,
        component_id: Arc<str>,
    ) -> Arc<Self> {
        let max = config.max_instances.max(1) as usize;
        Arc::new(Self {
            inner: Arc::new(PoolInner {
                engine,
                linker,
                component,
                config,
                component_id,
                idle: tokio::sync::Mutex::new(Vec::new()),
                live: AtomicU64::new(0),
                next_instance_id: AtomicU64::new(1),
                shutting_down: AtomicBool::new(false),
            }),
            checkout: Semaphore::new(max),
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.inner.shutting_down.load(Ordering::Acquire)
    }

    #[cfg(test)]
    #[must_use]
    pub fn available_checkouts(&self) -> usize {
        self.checkout.available_permits()
    }
}

fn store_reset_fuel(
    store: &mut wasmtime::Store<WasmStoreState>,
    fuel: u64,
) -> Result<(), WasmEngineError> {
    store
        .set_fuel(fuel)
        .map_err(|err| WasmEngineError::Internal(err.to_string()))
}

fn classify_trap(err: &wasmtime::Error) -> WasmFaultCode {
    if err.to_string().contains("all fuel consumed") {
        WasmFaultCode::FuelExhausted
    } else {
        WasmFaultCode::Trap
    }
}

fn to_wit_request_ctx(input: &RequestCtx) -> WitRequestCtx {
    WitRequestCtx {
        request_id: input.request_id.clone(),
        actor_id: input.actor_id.clone(),
        subject_id: input.subject_id.clone(),
        method: input.method.clone(),
        params_json: input.params_json.clone(),
        act_chain_json: input.act_chain_json.clone(),
        attributes_json: input.attributes_json.clone(),
        span: WitSpanContext {
            trace_id: input.span.trace_id.clone(),
            traceparent: input.span.traceparent.clone(),
            tracestate: input.span.tracestate.clone(),
        },
        tools: input
            .tools
            .iter()
            .map(|tool| WitToolDescriptor {
                name: tool.name.clone(),
                server_id: tool.server_id.clone(),
                description: tool.description.clone(),
                input_schema_json: tool.input_schema_json.clone(),
            })
            .collect(),
    }
}

fn from_wit_decision(decision: WitPolicyDecision) -> PolicyDecision {
    match decision {
        WitPolicyDecision::Allow => PolicyDecision::Allow,
        WitPolicyDecision::Deny(reason) => PolicyDecision::Deny(reason),
        WitPolicyDecision::Challenge(reason) => PolicyDecision::Challenge(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::bindings::RequestCtx;

    fn empty_component(engine: &Engine) -> Component {
        const MINIMAL: &[u8] = include_bytes!("testdata/minimal.component.wasm");
        Component::from_binary(engine, MINIMAL).expect("minimal component")
    }

    fn shell_pool(config: PoolConfig) -> Arc<ComponentPool> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.wasm_component_model(true);
        let engine = Engine::new(&wasm_config).expect("engine");
        let component = empty_component(&engine);
        let linker = Linker::new(&engine);
        ComponentPool::new_without_prewarm(
            engine,
            linker,
            component,
            config,
            Arc::from("policy"),
        )
    }

    #[tokio::test]
    async fn pool_semaphore_bounds_concurrency() {
        let checkout = Semaphore::new(2);
        let first = checkout.acquire().await.expect("first");
        let second = checkout.acquire().await.expect("second");
        let timed = tokio::time::timeout(Duration::from_millis(20), checkout.acquire()).await;
        assert!(timed.is_err(), "third acquire should time out");
        drop(first);
        drop(second);
    }

    #[tokio::test]
    async fn pool_drain_shuts_down_and_blocks_evaluate() {
        let pool = shell_pool(PoolConfig::for_tests());
        pool.drain().await;
        assert!(pool.is_shutting_down());
        assert_eq!(pool.idle_count(), 0);

        let request = RequestCtx {
            request_id: "req-1".into(),
            actor_id: "user:alice".into(),
            subject_id: "user:alice".into(),
            method: "tools/call".into(),
            params_json: "{}".into(),
            act_chain_json: "[]".into(),
            attributes_json: "{}".into(),
            span: crate::wasm::bindings::SpanContext {
                trace_id: "0".repeat(32),
                traceparent: "00-00000000000000000000000000000000-0000000000000000-01".into(),
                tracestate: None,
            },
            tools: vec![],
        };
        let err = pool
            .evaluate(&request, HostEvalContext::for_tests())
            .await
            .expect_err("evaluate after drain");
        assert!(matches!(err, WasmEngineError::PoolShutDown));
    }

    #[tokio::test]
    async fn pool_checkout_permit_released_after_evaluate() {
        let pool = shell_pool(PoolConfig {
            prewarm: 0,
            max_instances: 2,
            acquire_timeout_ms: 50,
            ..PoolConfig::for_tests()
        });
        let request = RequestCtx {
            request_id: "req-1".into(),
            actor_id: "user:alice".into(),
            subject_id: "user:alice".into(),
            method: "tools/call".into(),
            params_json: "{}".into(),
            act_chain_json: "[]".into(),
            attributes_json: "{}".into(),
            span: crate::wasm::bindings::SpanContext {
                trace_id: "0".repeat(32),
                traceparent: "00-00000000000000000000000000000000-0000000000000000-01".into(),
                tracestate: None,
            },
            tools: vec![],
        };
        let max = pool.available_checkouts();
        let _ = pool.evaluate(&request, HostEvalContext::for_tests()).await;
        assert_eq!(pool.available_checkouts(), max, "checkout permit released");
    }
}
