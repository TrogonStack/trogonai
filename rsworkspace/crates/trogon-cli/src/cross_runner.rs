use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use acp_nats::{AcpPrefix, Bridge, Config, NatsJetStreamClient};
use agent_client_protocol::{
    Agent as _, CloseSessionRequest, ExtRequest, NewSessionRequest, SessionNotification, SetSessionConfigOptionRequest,
    SetSessionModeRequest, SetSessionModelRequest,
};
use tokio::sync::mpsc;
use trogon_registry::{Registry, RegistryStore};
use trogon_std::time::SystemClock;
use trogonai_session_contracts::{
    IdempotencyKey, ModelSwitchReason, OperationId, SessionConfig, SessionId, SwitchResult,
};
use trogonai_session_kernel::{SessionKernelFeatureFlags, shadow_sync_from_export};
use trogonai_switching::{
    PassthroughCheckpointRunner, SwitchGateOutcome, SwitchModelRequest, SwitchOrchestrator, SwitchingError,
    classify_switch_result, messages_json_for_runner_hydration, portable_config_from_snapshot,
};

type ConcreteBridge = Bridge<async_nats::Client, SystemClock, NatsJetStreamClient>;

// LOW-7: Bundle each bridge with its notification receiver in a single tuple.
// The receiver is never polled here — CrossRunnerSwitcher only calls new_session
// and ext_method, which never trigger notifications.  Storing them as a pair
// makes it structurally impossible to drop the receiver without also dropping
// the bridge (two parallel HashMaps can silently diverge; one map of tuples
// cannot).
type BridgeSlot = (ConcreteBridge, mpsc::Receiver<SessionNotification>);

// ── RunnerSwitcher trait ──────────────────────────────────────────────────────

pub trait RunnerSwitcher {
    fn switch_model<'a>(
        &'a mut self,
        current_prefix: &'a str,
        current_session_id: &'a str,
        model_id: &'a str,
        cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<(String, String), String>> + 'a;
}

// ── CrossRunnerSwitcher ───────────────────────────────────────────────────────

pub struct CrossRunnerSwitcher<S: RegistryStore> {
    nats: async_nats::Client,
    base_config: Config,
    registry: Registry<S>,
    kernel_stack: Option<crate::session_kernel::SessionKernelStack>,
    /// Each entry is `(Bridge, notification_rx)`.  The receiver is kept alive
    /// for exactly as long as the bridge; they are inserted and removed together.
    bridges: HashMap<String, BridgeSlot>,
}

impl<S: RegistryStore> CrossRunnerSwitcher<S> {
    pub fn new(nats: async_nats::Client, base_config: Config, registry: Registry<S>) -> Self {
        Self {
            nats,
            base_config,
            registry,
            kernel_stack: None,
            bridges: HashMap::new(),
        }
    }

    pub fn with_kernel_stack(mut self, stack: crate::session_kernel::SessionKernelStack) -> Self {
        self.kernel_stack = Some(stack);
        self
    }

    pub fn kernel_flags(&self) -> SessionKernelFeatureFlags {
        self.kernel_stack
            .as_ref()
            .map(|stack| stack.flags.clone())
            .unwrap_or_default()
    }

    /// Switch the active session to whichever runner owns `model_id`.
    /// Returns `(target_prefix, new_session_id)`.
    /// Returns the original pair unchanged if the model is already on the current runner.
    ///
    /// Must be called from within a `tokio::task::LocalSet` — Bridge is `!Send`.
    pub async fn switch_model(
        &mut self,
        current_prefix: &str,
        current_session_id: &str,
        model_id: &str,
        cwd: &str,
    ) -> Result<(String, String), String> {
        // 1. Resolve target runner from registry
        let cap = self
            .registry
            .find_by_model(model_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no runner found for model: {model_id}"))?;
        let target_prefix = cap.metadata["acp_prefix"]
            .as_str()
            .ok_or("missing acp_prefix in registry metadata")?
            .to_string();
        if target_prefix == current_prefix {
            return Ok((current_prefix.to_string(), current_session_id.to_string()));
        }

        // 2. Ensure both bridges exist before any borrow
        self.ensure_bridge(current_prefix)?;
        self.ensure_bridge(&target_prefix)?;

        // 3. Export history as raw JSON (used for shadow mode and handoff fallback)
        let export_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": current_session_id }).to_string(),
        )
        .map_err(|e| e.to_string())?;
        let messages_json = {
            let (bridge, _) = self.bridges.get(current_prefix).unwrap();
            bridge
                .ext_method(ExtRequest::new("session/export", export_params.into()))
                .await
                .map_err(|e| e.to_string())?
                .0
        };
        let raw_messages = messages_json.get();

        let canonical_stack = self
            .kernel_stack
            .as_ref()
            .filter(|stack| stack.flags.use_canonical_runner_binding())
            .cloned();
        if let Some(stack) = canonical_stack {
            let started = Instant::now();
            match self
                .switch_via_session_kernel(
                    current_prefix,
                    current_session_id,
                    &target_prefix,
                    model_id,
                    cwd,
                    raw_messages,
                    &stack,
                )
                .await
            {
                Ok(result) => {
                    trogonai_switching::telemetry::metrics::record_switch_latency_ms(
                        current_session_id,
                        &result.operation_id,
                        started.elapsed().as_secs_f64() * 1000.0,
                        &result.source_model,
                        model_id,
                        current_prefix,
                        &target_prefix,
                        "completed",
                        result.checkpoint_result.as_deref(),
                    );
                    // The structured result is emitted (not a parsed string); a degraded
                    // or repaired switch stays visible in logs derived from it.
                    tracing::info!(
                        session_id = current_session_id,
                        target_model = model_id,
                        switch_result = switch_result_label(result.result),
                        "canonical model switch completed"
                    );
                    return Ok((target_prefix, result.runner_session_id));
                }
                Err(err) => {
                    trogonai_switching::telemetry::metrics::record_hydration_fallback(
                        current_session_id,
                        &err.operation_id,
                        &err.reason,
                    );
                    // § Rollback Strategy / Compatibilidad temporal: a silent handoff is
                    // only permitted while the event log is NOT primary. Under event-primary
                    // a canonical failure must surface the structured repairable result,
                    // never a quiet handoff that would fake continuity from the old runner.
                    if !stack.flags.allows_handoff_fallback() {
                        return Err(format!(
                            "model switch failed under event-primary mode ({}; no silent handoff): {}",
                            switch_result_label(err.result),
                            err.reason
                        ));
                    }
                    tracing::warn!(
                        session_id = current_session_id,
                        target_model = model_id,
                        switch_result = switch_result_label(err.result),
                        error = %err.reason,
                        "canonical session kernel switch failed — falling back to export/import handoff"
                    );
                }
            }
        }

        self.handoff_switch(&target_prefix, cwd, raw_messages).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn switch_via_session_kernel(
        &mut self,
        _current_prefix: &str,
        trogon_session_id: &str,
        target_prefix: &str,
        model_id: &str,
        cwd: &str,
        export_json: &str,
        stack: &crate::session_kernel::SessionKernelStack,
    ) -> Result<CanonicalSwitchOutcome, CanonicalSwitchFailure> {
        let session_id = SessionId::new(trogon_session_id).map_err(|err| CanonicalSwitchFailure {
            operation_id: trogon_session_id.to_string(),
            reason: err.to_string(),
            result: SwitchResult::FailedRecoverable,
        })?;
        let operation_id =
            OperationId::new(format!("op_switch_{}", uuid::Uuid::now_v7())).map_err(|err| CanonicalSwitchFailure {
                operation_id: trogon_session_id.to_string(),
                reason: err.to_string(),
                result: SwitchResult::FailedRecoverable,
            })?;
        let idempotency_key =
            IdempotencyKey::new(format!("idem_{}", uuid::Uuid::now_v7())).map_err(|err| CanonicalSwitchFailure {
                operation_id: operation_id.as_str().to_string(),
                reason: err.to_string(),
                result: SwitchResult::FailedRecoverable,
            })?;

        if stack.flags.event_log_shadow_mode {
            let portable_config = legacy_portable_session_config(&self.nats, trogon_session_id).await;
            let portable_snapshot = portable_config_to_session_config(&portable_config);
            if let Err(err) =
                shadow_sync_from_export(&stack.kernel, &session_id, export_json, cwd, portable_snapshot).await
            {
                tracing::warn!(
                    session_id = trogon_session_id,
                    error = %err,
                    "shadow event log sync failed — continuing with operational handoff path"
                );
            }
        }

        let orchestrator = SwitchOrchestrator::new(
            stack.kernel.clone(),
            stack.snapshots.clone(),
            stack.runner_bindings.clone(),
            stack.twin_store.clone(),
            self.registry.clone(),
            Arc::new(PassthroughCheckpointRunner),
            stack.switching_config.clone(),
            stack.capability_config.clone(),
            stack.projection_config.clone(),
            stack.certification.clone(),
        );

        let outcome = orchestrator
            .switch_model(SwitchModelRequest {
                session_id: session_id.clone(),
                target_model: model_id.to_string(),
                reason: ModelSwitchReason::UserRequested,
                user_confirmed: true,
                force: false,
                force_acknowledged_losses: Vec::new(),
                operation_id: operation_id.clone(),
                correlation_id: format!("corr_{}", uuid::Uuid::now_v7()),
                idempotency_key,
            })
            .await;
        // Normalize ONCE into the structured SwitchResult before unwrapping, so every
        // return path carries it (§ Trabajo restante: "emitir SwitchResult estructurado").
        let result_kind = classify_switch_result(&outcome);

        let switch_result = outcome.map_err(|err| map_switching_error(operation_id.as_str(), result_kind, err))?;

        let result = match switch_result {
            Ok(result) => result,
            Err(SwitchGateOutcome::Blocked(decision)) => {
                trogonai_switching::telemetry::metrics::record_switch_blocked(
                    trogon_session_id,
                    operation_id.as_str(),
                    model_id,
                    decision
                        .reasons
                        .first()
                        .map(|reason| reason.kind.as_str())
                        .unwrap_or("blocked"),
                );
                return Err(CanonicalSwitchFailure {
                    operation_id: operation_id.as_str().to_string(),
                    reason: "switch blocked by safety gate".to_string(),
                    result: result_kind,
                });
            }
            Err(SwitchGateOutcome::ConfirmationRequired(_)) => {
                trogonai_switching::telemetry::metrics::record_switch_confirmation_required(
                    trogon_session_id,
                    operation_id.as_str(),
                    model_id,
                );
                return Err(CanonicalSwitchFailure {
                    operation_id: operation_id.as_str().to_string(),
                    reason: "switch requires user confirmation".to_string(),
                    result: result_kind,
                });
            }
        };

        let portable = {
            let legacy = legacy_portable_session_config(&self.nats, trogon_session_id).await;
            let snapshot = stack
                .kernel
                .load_snapshot(&session_id)
                .await
                .map_err(|err| CanonicalSwitchFailure {
                    operation_id: operation_id.as_str().to_string(),
                    reason: err.to_string(),
                    // Canonical switch committed; only post-switch hydration failed, so the
                    // session stays consistent and the attempt is retryable.
                    result: SwitchResult::FailedRecoverable,
                })?;
            let mut portable = snapshot
                .as_ref()
                .and_then(|snap| snap.state.as_option())
                .map(portable_config_from_snapshot)
                .unwrap_or_default();
            if portable.compactor_model.is_none() {
                portable.compactor_model = legacy.compactor_model;
            }
            if portable.mode.is_none() {
                portable.mode = legacy.mode;
            }
            if portable.system_prompt.is_none() {
                portable.system_prompt = legacy.system_prompt;
            }
            if portable.mcp_servers_json.is_empty() {
                portable.mcp_servers_json = legacy.mcp_servers_json;
            }
            portable
        };

        let import_messages =
            messages_json_for_runner_hydration(&result.projection, &[]).map_err(|err| CanonicalSwitchFailure {
                operation_id: operation_id.as_str().to_string(),
                reason: err.to_string(),
                result: SwitchResult::FailedRecoverable,
            })?;

        let new_session_id = self
            .open_and_hydrate_runner_session(target_prefix, cwd, &import_messages, model_id, &portable)
            .await
            .map_err(|reason| CanonicalSwitchFailure {
                operation_id: operation_id.as_str().to_string(),
                reason,
                result: SwitchResult::FailedRecoverable,
            })?;

        trogonai_switching::telemetry::metrics::record_switch_completed(
            trogon_session_id,
            operation_id.as_str(),
            &result.from_model,
            &result.to_model,
            &result.from_runner,
            &result.to_runner,
        );

        let checkpoint_result = result
            .checkpoint
            .as_ref()
            .map(|checkpoint| match checkpoint.status.as_known() {
                Some(trogonai_session_contracts::ContinuityCheckpointStatus::Passed) => "passed",
                Some(trogonai_session_contracts::ContinuityCheckpointStatus::Repaired) => "repaired",
                Some(trogonai_session_contracts::ContinuityCheckpointStatus::Failed) => "failed",
                _ => "unknown",
            });

        Ok(CanonicalSwitchOutcome {
            operation_id: operation_id.as_str().to_string(),
            source_model: result.from_model,
            runner_session_id: new_session_id,
            checkpoint_result: checkpoint_result.map(str::to_string),
            result: result_kind,
        })
    }

    async fn handoff_switch(
        &mut self,
        target_prefix: &str,
        cwd: &str,
        raw_messages: &str,
    ) -> Result<(String, String), String> {
        if raw_messages.trim() == "null" || raw_messages.trim().is_empty() {
            return Err("session/export returned null — cannot import into new session".into());
        }

        let new_session_id = {
            let (bridge, _) = self.bridges.get(target_prefix).unwrap();
            bridge
                .new_session(NewSessionRequest::new(cwd))
                .await
                .map_err(|e| e.to_string())?
                .session_id
                .to_string()
        };

        let import_params = serde_json::value::RawValue::from_string(format!(
            r#"{{"sessionId":"{new_session_id}","messages":{raw_messages}}}"#
        ))
        .map_err(|e| e.to_string())?;
        {
            let (bridge, _) = self.bridges.get(target_prefix).unwrap();
            if let Err(import_err) = bridge
                .ext_method(ExtRequest::new("session/import", import_params.into()))
                .await
            {
                let _ = bridge
                    .close_session(CloseSessionRequest::new(new_session_id.clone()))
                    .await;
                return Err(import_err.to_string());
            }
        }

        Ok((target_prefix.to_string(), new_session_id))
    }

    async fn open_and_hydrate_runner_session(
        &mut self,
        target_prefix: &str,
        cwd: &str,
        import_messages_json: &str,
        model_id: &str,
        portable: &trogonai_switching::PortableRunnerConfig,
    ) -> Result<String, String> {
        let new_session_id = {
            let (bridge, _) = self.bridges.get(target_prefix).unwrap();
            bridge
                .new_session(NewSessionRequest::new(cwd))
                .await
                .map_err(|e| e.to_string())?
                .session_id
                .to_string()
        };

        let import_params = serde_json::value::RawValue::from_string(format!(
            r#"{{"sessionId":"{new_session_id}","messages":{import_messages_json}}}"#
        ))
        .map_err(|e| e.to_string())?;
        {
            let (bridge, _) = self.bridges.get(target_prefix).unwrap();
            if let Err(import_err) = bridge
                .ext_method(ExtRequest::new("session/import", import_params.into()))
                .await
            {
                let _ = bridge
                    .close_session(CloseSessionRequest::new(new_session_id.clone()))
                    .await;
                return Err(import_err.to_string());
            }
            apply_portable_runner_config(bridge, &new_session_id, model_id, portable).await?;
        }

        Ok(new_session_id)
    }

    fn ensure_bridge(&mut self, prefix: &str) -> Result<(), String> {
        if self.bridges.contains_key(prefix) {
            return Ok(());
        }
        let acp_prefix = AcpPrefix::new(prefix).map_err(|e| e.to_string())?;
        let config = self.base_config.for_prefix(acp_prefix);
        let js = NatsJetStreamClient::new(async_nats::jetstream::new(self.nats.clone()));
        let (notification_tx, notification_rx) = mpsc::channel(1);
        let bridge = Bridge::new(
            self.nats.clone(),
            js,
            SystemClock,
            &opentelemetry::global::meter("trogon-cli"),
            config,
            notification_tx,
        );
        self.bridges.insert(prefix.to_string(), (bridge, notification_rx));
        Ok(())
    }
}

impl<S: RegistryStore> RunnerSwitcher for CrossRunnerSwitcher<S> {
    fn switch_model<'a>(
        &'a mut self,
        current_prefix: &'a str,
        current_session_id: &'a str,
        model_id: &'a str,
        cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<(String, String), String>> + 'a {
        CrossRunnerSwitcher::switch_model(self, current_prefix, current_session_id, model_id, cwd)
    }
}

// ── MockRunnerSwitcher (test support) ─────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::RunnerSwitcher;

    pub struct MockRunnerSwitcher {
        result: Result<(String, String), String>,
    }

    impl MockRunnerSwitcher {
        pub fn same_runner(prefix: &str, session_id: &str) -> Self {
            Self {
                result: Ok((prefix.to_string(), session_id.to_string())),
            }
        }
        pub fn cross_runner(new_prefix: &str, new_session_id: &str) -> Self {
            Self {
                result: Ok((new_prefix.to_string(), new_session_id.to_string())),
            }
        }
        pub fn error(msg: &str) -> Self {
            Self {
                result: Err(msg.to_string()),
            }
        }
    }

    impl RunnerSwitcher for MockRunnerSwitcher {
        fn switch_model<'a>(
            &'a mut self,
            _current_prefix: &'a str,
            _current_session_id: &'a str,
            _model_id: &'a str,
            _cwd: &'a str,
        ) -> impl std::future::Future<Output = Result<(String, String), String>> + 'a {
            let result = self.result.clone();
            async move { result }
        }
    }
}

struct CanonicalSwitchOutcome {
    operation_id: String,
    source_model: String,
    runner_session_id: String,
    checkpoint_result: Option<String>,
    /// The normalized, structured switch result (§ Contrato formal de resultado del
    /// switch). cross_runner emits THIS, not a parsed string.
    result: SwitchResult,
}

struct CanonicalSwitchFailure {
    operation_id: String,
    reason: String,
    /// Structured outcome so the caller/UX derive behavior from the result, not text
    /// (§ "logs y metricas deben derivarse del resultado estructurado, no de parsear texto").
    result: SwitchResult,
}

fn map_switching_error(operation_id: &str, result: SwitchResult, err: SwitchingError) -> CanonicalSwitchFailure {
    CanonicalSwitchFailure {
        operation_id: operation_id.to_string(),
        reason: err.to_string(),
        result,
    }
}

/// Stable snake_case label for the structured switch result, for user-facing messages
/// and logs that must derive from the result rather than parse free text.
fn switch_result_label(result: SwitchResult) -> &'static str {
    match result {
        SwitchResult::Unspecified => "unspecified",
        SwitchResult::Switched => "switched",
        SwitchResult::Blocked => "blocked",
        SwitchResult::RequiresConfirmation => "requires_confirmation",
        SwitchResult::Degraded => "degraded",
        SwitchResult::Repaired => "repaired",
        SwitchResult::RolledBack => "rolled_back",
        SwitchResult::FailedRecoverable => "failed_recoverable",
        SwitchResult::FailedTerminal => "failed_terminal",
    }
}

async fn legacy_portable_session_config(
    nats: &async_nats::Client,
    session_id: &str,
) -> trogonai_switching::PortableRunnerConfig {
    use trogon_runner_tools::SessionStore as _;
    let js = async_nats::jetstream::new(nats.clone());
    match trogon_runner_tools::NatsSessionStore::open(&js).await {
        Ok(store) => match store.load(session_id).await {
            Ok(state) => trogonai_switching::PortableRunnerConfig {
                compactor_model: state.compactor_model.clone(),
                mode: Some(state.mode.clone()),
                system_prompt: state.system_prompt.clone(),
                mcp_servers_json: state
                    .mcp_servers
                    .iter()
                    .filter_map(|server| serde_json::to_string(server).ok())
                    .collect(),
            },
            Err(_) => trogonai_switching::PortableRunnerConfig::default(),
        },
        Err(_) => trogonai_switching::PortableRunnerConfig::default(),
    }
}

fn portable_config_to_session_config(portable: &trogonai_switching::PortableRunnerConfig) -> Option<SessionConfig> {
    if portable.compactor_model.is_none() && portable.system_prompt.is_none() && portable.mcp_servers_json.is_empty() {
        return None;
    }
    Some(SessionConfig {
        compactor_model: portable.compactor_model.clone(),
        system_prompt: portable.system_prompt.clone(),
        mcp_servers_json: portable.mcp_servers_json.clone(),
        ..SessionConfig::default()
    })
}

async fn apply_portable_runner_config(
    bridge: &ConcreteBridge,
    session_id: &str,
    model_id: &str,
    portable: &trogonai_switching::PortableRunnerConfig,
) -> Result<(), String> {
    bridge
        .set_session_model(SetSessionModelRequest::new(
            session_id.to_string(),
            model_id.to_string(),
        ))
        .await
        .map_err(|err| err.to_string())?;
    if let Some(mode) = &portable.mode {
        bridge
            .set_session_mode(SetSessionModeRequest::new(session_id.to_string(), mode.to_string()))
            .await
            .map_err(|err| err.to_string())?;
    }
    if let Some(compactor_model) = &portable.compactor_model {
        bridge
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                session_id.to_string(),
                "compactor_model",
                compactor_model.as_str(),
            ))
            .await
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_nats::AcpPrefix;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;
    use trogon_nats::{NatsAuth, NatsConfig};
    use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

    async fn start_nats() -> (testcontainers_modules::testcontainers::ContainerAsync<Nats>, u16) {
        let container = Nats::default()
            .start()
            .await
            .expect("NATS container failed — is Docker running?");
        let port = container.get_host_port_ipv4(4222).await.unwrap();
        (container, port)
    }

    fn make_config(port: u16) -> Config {
        let nats_cfg = NatsConfig {
            servers: vec![format!("127.0.0.1:{port}")],
            auth: NatsAuth::None,
        };
        Config::new(AcpPrefix::new("acp").unwrap(), nats_cfg)
    }

    async fn connect(port: u16) -> async_nats::Client {
        async_nats::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("failed to connect to NATS")
    }

    fn cap_with_prefix(model: &str, acp_prefix: &str) -> AgentCapability {
        let mut cap = AgentCapability::new("runner", ["chat"], "agents.runner.>");
        cap.metadata = serde_json::json!({ "models": [model], "acp_prefix": acp_prefix });
        cap
    }

    // §2206 "emitir SwitchResult estructurado": cross_runner carries the structured
    // result through its failure type, and user-facing text DERIVES from it (§2031:
    // "logs y metricas deben derivarse del resultado estructurado, no de parsear texto").

    #[test]
    fn map_switching_error_carries_the_structured_result() {
        let failure = map_switching_error(
            "op_1",
            SwitchResult::FailedTerminal,
            SwitchingError::TargetModelNotFound { model_id: "ghost".to_string() },
        );
        assert_eq!(failure.result, SwitchResult::FailedTerminal);
        assert_eq!(failure.operation_id, "op_1");
    }

    #[test]
    fn switch_result_label_is_stable_for_every_state() {
        assert_eq!(switch_result_label(SwitchResult::Switched), "switched");
        assert_eq!(switch_result_label(SwitchResult::Blocked), "blocked");
        assert_eq!(switch_result_label(SwitchResult::RequiresConfirmation), "requires_confirmation");
        assert_eq!(switch_result_label(SwitchResult::Degraded), "degraded");
        assert_eq!(switch_result_label(SwitchResult::Repaired), "repaired");
        assert_eq!(switch_result_label(SwitchResult::RolledBack), "rolled_back");
        assert_eq!(switch_result_label(SwitchResult::FailedRecoverable), "failed_recoverable");
        assert_eq!(switch_result_label(SwitchResult::FailedTerminal), "failed_terminal");
    }

    #[tokio::test]
    async fn same_runner_returns_unchanged() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.test")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher
            .switch_model("acp.test", "session-abc", "gpt-4", "/workspace")
            .await;
        assert_eq!(result, Ok(("acp.test".to_string(), "session-abc".to_string())));
    }

    #[tokio::test]
    async fn model_not_found_returns_error() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher
            .switch_model("acp.current", "session-1", "unknown-model", "/ws")
            .await;
        assert_eq!(result, Err("no runner found for model: unknown-model".to_string()));
    }

    #[tokio::test]
    async fn missing_acp_prefix_returns_error() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());
        let mut cap = AgentCapability::new("runner", ["chat"], "agents.runner.>");
        cap.metadata = serde_json::json!({ "models": ["gpt-4"] });
        registry.register(&cap).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.other", "session-1", "gpt-4", "/ws").await;
        assert_eq!(result, Err("missing acp_prefix in registry metadata".to_string()));
    }

    #[tokio::test]
    async fn invalid_target_prefix_returns_error() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());
        // "acp..bad" has consecutive dots — AcpPrefix::new rejects it
        registry.register(&cap_with_prefix("gpt-4", "acp..bad")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.current", "session-1", "gpt-4", "/ws").await;
        assert!(result.is_err(), "expected Err but got: {result:?}");
    }

    // ── Happy path: full cross-runner migration ───────────────────────────────

    /// Sets up a NATS subscriber that responds once to `subject` with `response_bytes`.
    async fn mock_responder(nats: async_nats::Client, subject: &'static str, response_bytes: &'static [u8]) {
        use futures::StreamExt as _;
        let mut sub = nats.subscribe(subject).await.expect("subscribe");
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await
                && let Some(reply) = msg.reply
            {
                nats.publish(reply, axum::body::Bytes::from_static(response_bytes))
                    .await
                    .ok();
            }
        });
    }

    #[tokio::test]
    async fn cross_runner_migration_succeeds() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        // session/export → any valid JSON (ExtResponse is transparent)
        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", b"[]").await;
        // new_session → {"sessionId":"migrated-session"}
        mock_responder(
            nats_bg.clone(),
            "acp.tgt.agent.session.new",
            br#"{"sessionId":"migrated-session"}"#,
        )
        .await;
        // session/import → any valid JSON
        mock_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", b"{}").await;

        // small delay so all subscriptions are registered before the bridge sends requests
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher
            .switch_model("acp.src", "old-session-id", "gpt-4", "/workspace")
            .await;

        assert_eq!(result, Ok(("acp.tgt".to_string(), "migrated-session".to_string())));
    }

    // ── Error propagation ─────────────────────────────────────────────────────

    /// Like `mock_responder` but handles `count` consecutive messages on the same subject.
    async fn multi_responder(nats: async_nats::Client, subject: &str, responses: Vec<&'static [u8]>) {
        use futures::StreamExt as _;
        let mut sub = nats.subscribe(subject.to_string()).await.expect("subscribe");
        tokio::spawn(async move {
            for response in responses {
                if let Some(msg) = sub.next().await
                    && let Some(reply) = msg.reply
                {
                    nats.publish(reply, axum::body::Bytes::from_static(response)).await.ok();
                }
            }
        });
    }

    /// Subscribes to `subject`, captures the first request payload, and replies with `response_bytes`.
    async fn capturing_responder(
        nats: async_nats::Client,
        subject: &str,
        response_bytes: &'static [u8],
    ) -> tokio::sync::oneshot::Receiver<Vec<u8>> {
        use futures::StreamExt as _;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut sub = nats.subscribe(subject.to_string()).await.expect("subscribe");
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                let payload = msg.payload.to_vec();
                if let Some(reply) = msg.reply {
                    nats.publish(reply, axum::body::Bytes::from_static(response_bytes))
                        .await
                        .ok();
                }
                tx.send(payload).ok();
            }
        });
        rx
    }

    #[tokio::test]
    async fn export_failure_propagates_error() {
        let (_container, port) = start_nats().await;
        // No responder for session/export → NATS returns "no responders" immediately
        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.src", "session-1", "gpt-4", "/ws").await;
        assert!(result.is_err(), "expected export failure to propagate; got: {result:?}");
    }

    #[tokio::test]
    async fn new_session_failure_propagates_error() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", b"[]").await;
        // No responder for acp.tgt.agent.session.new → error
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.src", "session-1", "gpt-4", "/ws").await;
        assert!(
            result.is_err(),
            "expected new_session failure to propagate; got: {result:?}"
        );
    }

    #[tokio::test]
    async fn import_failure_propagates_error() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", b"[]").await;
        mock_responder(nats_bg.clone(), "acp.tgt.agent.session.new", br#"{"sessionId":"s1"}"#).await;
        // No responder for session/import → error
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.src", "session-1", "gpt-4", "/ws").await;
        assert!(result.is_err(), "expected import failure to propagate; got: {result:?}");
    }

    // ── Bridge reuse ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn bridge_is_reused_across_switch_model_calls() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        // Each subject handles 2 consecutive messages (one per switch_model call).
        // Using multi_responder avoids the race where two separate subscribers both
        // receive the first request and leave the second call without a responder.
        multi_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", vec![b"[]", b"[]"]).await;
        multi_responder(
            nats_bg.clone(),
            "acp.tgt.agent.session.new",
            vec![br#"{"sessionId":"s1"}"#, br#"{"sessionId":"s2"}"#],
        )
        .await;
        multi_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", vec![b"{}", b"{}"]).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);

        let r1 = switcher.switch_model("acp.src", "old-1", "gpt-4", "/ws").await;
        assert_eq!(r1, Ok(("acp.tgt".to_string(), "s1".to_string())));

        let r2 = switcher.switch_model("acp.src", "old-2", "gpt-4", "/ws").await;
        assert_eq!(r2, Ok(("acp.tgt".to_string(), "s2".to_string())));
    }

    // ── Import payload correctness ────────────────────────────────────────────

    /// Verifies that the `cwd` argument passed to `switch_model` is forwarded
    /// verbatim to `new_session` on the target runner — CrossRunnerSwitcher must
    /// not silently drop or replace the workspace path.
    #[tokio::test]
    async fn cwd_is_passed_to_target_new_session() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", b"[]").await;
        // Capture the new_session request payload so we can inspect its cwd field.
        let new_session_rx = capturing_responder(
            nats_bg.clone(),
            "acp.tgt.agent.session.new",
            br#"{"sessionId":"cwd-check-session"}"#,
        )
        .await;
        mock_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", b"{}").await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        switcher
            .switch_model("acp.src", "session-1", "gpt-4", "/workspace/myproject")
            .await
            .unwrap();

        let new_session_body = new_session_rx
            .await
            .expect("new_session request must have been captured");
        let json: serde_json::Value = serde_json::from_slice(&new_session_body).unwrap();
        assert_eq!(
            json["cwd"].as_str(),
            Some("/workspace/myproject"),
            "cwd must be forwarded verbatim to new_session on the target runner; got: {json}"
        );
    }

    #[tokio::test]
    async fn import_payload_contains_exported_messages() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        // Export returns a concrete messages array
        mock_responder(
            nats_bg.clone(),
            "acp.src.agent.ext.session/export",
            br#"[{"role":"user","content":"hello"}]"#,
        )
        .await;
        mock_responder(
            nats_bg.clone(),
            "acp.tgt.agent.session.new",
            br#"{"sessionId":"new-sess"}"#,
        )
        .await;
        // Capture the import request so we can inspect its body
        let import_rx = capturing_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", b"{}").await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        switcher
            .switch_model("acp.src", "src-session", "gpt-4", "/ws")
            .await
            .unwrap();

        let import_body = import_rx.await.expect("import responder captured payload");
        let json: serde_json::Value = serde_json::from_slice(&import_body).unwrap();
        assert_eq!(json["sessionId"], "new-sess");
        assert_eq!(
            json["messages"],
            serde_json::json!([{"role": "user", "content": "hello"}])
        );
    }

    #[tokio::test]
    async fn import_payload_passes_v2_export_unchanged() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        let v2_export =
            br#"{"version":2,"messages":[{"version":2,"role":"user","blocks":[{"type":"text","text":"hi"}]}]}"#;
        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", v2_export).await;
        mock_responder(
            nats_bg.clone(),
            "acp.tgt.agent.session.new",
            br#"{"sessionId":"v2-sess"}"#,
        )
        .await;
        let import_rx = capturing_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", b"{}").await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        switcher
            .switch_model("acp.src", "src-session", "gpt-4", "/ws")
            .await
            .unwrap();

        let import_body = import_rx.await.expect("import responder captured payload");
        let json: serde_json::Value = serde_json::from_slice(&import_body).unwrap();
        assert_eq!(json["sessionId"], "v2-sess");
        assert_eq!(json["messages"]["version"], 2);
        // Verify the text block is parseable by the import handler
        let block = &json["messages"]["messages"][0]["blocks"][0];
        assert_eq!(block["type"], "text");
        assert_eq!(block["text"], "hi");
    }
}
