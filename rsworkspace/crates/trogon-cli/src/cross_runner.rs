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
    IdempotencyKey, ModelSwitchReason, OperationId, PromptProjection, SessionConfig, SessionId, SwitchResult,
    SwitchVisibleResult,
};
use trogonai_session_kernel::{SessionKernelFeatureFlags, resolve_event_log_primary, shadow_sync_from_export};
use trogonai_switching::{
    JsonAcknowledgementRunner, RunnerAcknowledgement, SwitchCompletion, SwitchGateOutcome, SwitchModelRequest,
    SwitchOrchestrator, SwitchingError, VisibleResultContext, build_visible_result, classify_switch_result,
    failed_visible_result, handoff_visible_result, messages_json_for_runner_hydration, portable_config_from_snapshot,
    same_runner_visible_result,
};

use time::OffsetDateTime;
use trogonai_capabilities::{CertificationLevel, RunnerCapabilityProbe};

use trogonai_switching::{CancelContext, CancelOutcome, cancel_operation, tool_states_from_session};

use crate::cancellation::SessionRunnerCancellation;
use crate::capability_probe::SessionCapabilityProbe;
use crate::checkpoint_transport::SessionAckTransport;

type ConcreteBridge = Bridge<async_nats::Client, SystemClock, NatsJetStreamClient>;

// LOW-7: Bundle each bridge with its notification receiver in a single tuple.
// The receiver is never polled here — CrossRunnerSwitcher only calls new_session
// and ext_method, which never trigger notifications.  Storing them as a pair
// makes it structurally impossible to drop the receiver without also dropping
// the bridge (two parallel HashMaps can silently diverge; one map of tuples
// cannot).
type BridgeSlot = (ConcreteBridge, mpsc::Receiver<SessionNotification>);

// ── RunnerSwitcher trait ──────────────────────────────────────────────────────

/// The single surface every caller of a model switch consumes (§ Contrato de
/// resultado visible del switch, §2043-2086). It carries the binding to attach to plus
/// the durable [`SwitchVisibleResult`] so CLI/TUI/events render the SAME contract and no
/// surface can fake continuity (§2084, §2289). `new_prefix == current_prefix` means the
/// active binding did not change (same-runner in-place model set).
#[derive(Debug, PartialEq)]
pub struct SwitchSurface {
    pub new_prefix: String,
    pub new_session_id: String,
    pub visible: SwitchVisibleResult,
}

pub trait RunnerSwitcher {
    fn switch_model<'a>(
        &'a mut self,
        current_prefix: &'a str,
        current_session_id: &'a str,
        current_model: &'a str,
        model_id: &'a str,
        cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<SwitchSurface, String>> + 'a;

    /// Reconstruct + hydrate an event-log-primary session on resume (Fase 11). The
    /// default treats every session as legacy (returns `None`), so callers fall back to
    /// the runner's own `load_session`; the canonical switcher overrides it.
    fn resume_event_primary<'a>(
        &'a mut self,
        _prefix: &'a str,
        _session_id: &'a str,
        _cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<Option<String>, String>> + 'a {
        async { Ok(None) }
    }

    /// Mark a freshly created session as event-log-primary per the configured mode
    /// (Fase 11). Default is a no-op for switchers without a kernel.
    fn mark_session_event_primary<'a>(
        &'a self,
        _session_id: &'a str,
    ) -> impl std::future::Future<Output = Result<(), String>> + 'a {
        async { Ok(()) }
    }

    /// Run the capability-probe battery against `model` on `prefix` and certify it from
    /// the verified results (§ Capability Registry Freshness), returning the resulting
    /// certification level label. Default: unsupported (no kernel).
    fn certify_model<'a>(
        &'a mut self,
        _prefix: &'a str,
        _model: &'a str,
        _cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<String, String>> + 'a {
        async { Err("capability probing is not supported by this switcher".to_string()) }
    }
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

    /// Drive the canonical cancellation flow for an in-flight kernel operation
    /// (§ Cancellation and Abort Semantics): request cancel, ask the runner to stop via the
    /// ACP cancel, then append the terminal cancel events (`operation_cancelled` /
    /// `operation_cancel_failed` / `operation_requires_reconciliation`) to the event log —
    /// so a caller only releases the Session Lease after a terminal state is durably
    /// recorded. Tool execution states come from the canonical snapshot, deciding
    /// cancel-before-tools vs await-runner-receipt. Returns the structured outcome.
    pub async fn cancel_session_operation(
        &self,
        prefix: &str,
        session_id: &str,
        operation_id: &str,
    ) -> Result<CancelOutcome, String> {
        let stack = self
            .kernel_stack
            .as_ref()
            .ok_or_else(|| "session kernel not active — cannot cancel canonically".to_string())?;
        let sid = SessionId::new(session_id).map_err(|err| err.to_string())?;
        let op = OperationId::new(operation_id).map_err(|err| err.to_string())?;
        let idempotency_key =
            IdempotencyKey::new(format!("idem_cancel_{}", uuid::Uuid::now_v7())).map_err(|err| err.to_string())?;

        let tool_states = stack
            .kernel
            .load_snapshot(&sid)
            .await
            .map_err(|err| err.to_string())?
            .as_ref()
            .and_then(|snapshot| snapshot.state.as_option())
            .map(|state| tool_states_from_session(&state.tool_calls))
            .unwrap_or_default();

        let context = CancelContext::for_cli(
            sid,
            op,
            format!("corr_cancel_{}", uuid::Uuid::now_v7()),
            idempotency_key,
            prefix.to_string(),
            OffsetDateTime::now_utc().unix_timestamp(),
        );
        let cancellation =
            SessionRunnerCancellation::new(self.nats.clone(), prefix.to_string(), session_id.to_string());

        let (outcome, _state, events) = cancel_operation(Some(&cancellation), &context, &tool_states)
            .await
            .map_err(|err| err.to_string())?;
        // Durably record every terminal cancel event before the caller releases the lease
        // ("no liberar Session Lease hasta registrar cancelled/failed/requires_reconciliation").
        for event in events {
            stack.kernel.append_event(event).await.map_err(|err| err.to_string())?;
        }
        Ok(outcome)
    }

    /// Run the live capability-probe battery against `model` on `prefix` and certify the
    /// model in the kernel stack's certification matrix from the verified results
    /// (§ Capability Registry Freshness: probes promote a model out of the unverified
    /// `Basic` baseline; only what was actually verified is certified). Returns the
    /// resulting certification level. The upgraded matrix is then used by the Switch
    /// Safety Gate on subsequent switches.
    pub async fn probe_and_certify(
        &mut self,
        prefix: &str,
        model: &str,
        cwd: &str,
    ) -> Result<CertificationLevel, String> {
        // Resolve the runner id (agent type) the certification matrix is keyed by, from the
        // registry; fall back to the prefix tail if the model is not registered.
        let runner_id = self
            .registry
            .find_by_model(model)
            .await
            .map_err(|err| err.to_string())?
            .map(|cap| cap.agent_type)
            .unwrap_or_else(|| prefix.trim_start_matches("acp.").to_string());

        let probe = SessionCapabilityProbe::new(
            self.nats.clone(),
            prefix.to_string(),
            model.to_string(),
            std::path::PathBuf::from(cwd),
        );
        let results = RunnerCapabilityProbe::new(probe).run_battery().await;
        let now = OffsetDateTime::now_utc();
        let stack = self
            .kernel_stack
            .as_mut()
            .ok_or_else(|| "session kernel not active — cannot certify".to_string())?;
        let level = stack
            .certification
            .certify_from_probes(model, &runner_id, &results, now)
            .map_err(|err| err.to_string())?;
        stack
            .certification_store
            .save_matrix(&stack.certification)
            .await
            .map_err(|err| err.to_string())?;
        Ok(level)
    }

    async fn certify_model_label(&mut self, prefix: &str, model: &str, cwd: &str) -> Result<String, String> {
        let level = self.probe_and_certify(prefix, model, cwd).await?;
        Ok(format!("{level:?}"))
    }

    /// Mark a freshly created session's routing record per the configured
    /// event-log-primary mode (Fase 11). No-op when the kernel stack is absent.
    async fn mark_session_event_primary_inner(&self, session_id: &str) -> Result<(), String> {
        let Some(stack) = self.kernel_stack.as_ref() else {
            return Ok(());
        };
        let sid = SessionId::new(session_id).map_err(|e| e.to_string())?;
        stack
            .migration
            .mark_new_session(&sid, &stack.flags)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// If `session_id` is event-log-primary, reconstruct its canonical conversation from
    /// the event log and produce `session/import` messages JSON to hydrate a runner
    /// (Fase 11: "Usar replay + snapshots como fuente de verdad para sesiones opt-in").
    /// Returns `None` when the kernel is inactive, the session has no routing record, or
    /// the mode is legacy — callers then fall back to the runner's own load_session.
    pub async fn recover_event_primary_messages(&self, session_id: &str) -> Result<Option<String>, String> {
        let Some(stack) = self.kernel_stack.as_ref() else {
            return Ok(None);
        };
        let sid = SessionId::new(session_id).map_err(|e| e.to_string())?;
        let Some(record) = stack.migration.load_routing(&sid).await.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        if !resolve_event_log_primary(&stack.flags, &record) {
            return Ok(None);
        }
        let recovered = stack.kernel.recover(&sid).await.map_err(|e| e.to_string())?;
        let conversation = recovered
            .snapshot
            .state
            .as_option()
            .map(|state| state.conversation.clone())
            .unwrap_or_default();
        // No projection for resume: hydrate from the full canonical conversation.
        let messages = messages_json_for_runner_hydration(&PromptProjection::default(), &conversation)
            .map_err(|e| e.to_string())?;
        Ok(Some(messages))
    }

    /// Resume an event-log-primary session by reconstructing it from the kernel and
    /// hydrating a fresh runner session. Returns the new runner session id, or `None`
    /// when the session is not event-log-primary (caller uses the legacy resume path).
    async fn resume_event_primary_inner(
        &mut self,
        prefix: &str,
        session_id: &str,
        cwd: &str,
    ) -> Result<Option<String>, String> {
        let Some(messages) = self.recover_event_primary_messages(session_id).await? else {
            return Ok(None);
        };
        let new_session_id = self.hydrate_new_runner_session(prefix, cwd, &messages).await?;
        Ok(Some(new_session_id))
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
        current_model: &str,
        model_id: &str,
        cwd: &str,
    ) -> Result<SwitchSurface, String> {
        self.switch_model_for_session(
            current_prefix,
            current_session_id,
            current_session_id,
            current_model,
            model_id,
            cwd,
        )
        .await
    }

    /// Switch a model while keeping `trogon_session_id` as the canonical visible
    /// session identity and using `current_runner_session_id` only to talk to the
    /// currently attached runtime binding.
    ///
    /// ACP multi-runner sessions have two IDs once routed externally: the IDE-visible
    /// Trogon session id (`acp_sid`) and the runner-local binding id (`runner_sid`).
    /// The Session Kernel must use the former for events/snapshots/leases; NATS
    /// `session/export` must use the latter. The plain CLI path passes the same value
    /// for both, preserving existing behavior.
    pub async fn switch_model_for_session(
        &mut self,
        current_prefix: &str,
        current_runner_session_id: &str,
        trogon_session_id: &str,
        current_model: &str,
        model_id: &str,
        cwd: &str,
    ) -> Result<SwitchSurface, String> {
        self.switch_model_for_session_into(
            current_prefix,
            current_runner_session_id,
            trogon_session_id,
            None,
            current_model,
            model_id,
            cwd,
        )
        .await
    }

    /// Variant of [`Self::switch_model_for_session`] that hydrates a known target
    /// runner session id instead of opening a new binding. ACP uses this when switching
    /// back to the embedded runner: the target binding is the IDE-visible `acp_sid`, not
    /// a new session.
    pub async fn switch_model_for_session_into(
        &mut self,
        current_prefix: &str,
        current_runner_session_id: &str,
        trogon_session_id: &str,
        target_runner_session_id: Option<&str>,
        current_model: &str,
        model_id: &str,
        cwd: &str,
    ) -> Result<SwitchSurface, String> {
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
            // Same runner: an in-place model set, the active binding is unchanged.
            return Ok(SwitchSurface {
                new_prefix: current_prefix.to_string(),
                new_session_id: current_runner_session_id.to_string(),
                visible: same_runner_visible_result(trogon_session_id, current_model, model_id, current_prefix),
            });
        }

        // 2. Ensure both bridges exist before any borrow
        self.ensure_bridge(current_prefix)?;
        self.ensure_bridge(&target_prefix)?;

        // 3. Export history as raw JSON (used for shadow mode and handoff fallback)
        let export_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": current_runner_session_id }).to_string(),
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
                    trogon_session_id,
                    current_model,
                    &target_prefix,
                    target_runner_session_id,
                    model_id,
                    cwd,
                    raw_messages,
                    &stack,
                )
                .await
            {
                Ok(result) => {
                    trogonai_switching::telemetry::metrics::record_switch_latency_ms(
                        trogon_session_id,
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
                        session_id = trogon_session_id,
                        target_model = model_id,
                        switch_result = switch_result_label(result.result),
                        "canonical model switch completed"
                    );
                    return Ok(SwitchSurface {
                        new_prefix: target_prefix,
                        new_session_id: result.runner_session_id,
                        visible: result.visible,
                    });
                }
                Err(err) => {
                    trogonai_switching::telemetry::metrics::record_hydration_fallback(
                        trogon_session_id,
                        &err.operation_id,
                        &err.reason,
                    );
                    // § Switch Safety Gate / §2289: a blocked or confirmation-required gate
                    // result is NOT a transport failure — it must surface as the structured
                    // result with its next_action, and it must NEVER fall through to a silent
                    // handoff (that would fake the very continuity the gate refused). The
                    // visible result already carries the reasons/next_action; render them.
                    if matches!(err.result, SwitchResult::Blocked | SwitchResult::RequiresConfirmation) {
                        return Err(blocked_surface_message(&err.visible));
                    }
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
                        session_id = trogon_session_id,
                        target_model = model_id,
                        switch_result = switch_result_label(err.result),
                        error = %err.reason,
                        "canonical session kernel switch failed — falling back to export/import handoff"
                    );
                    // The handoff that follows is a fallback FROM an attempted canonical
                    // switch: surface fallback_used=true with the canonical failure reason.
                    return self
                        .handoff_switch(
                            trogon_session_id,
                            current_model,
                            current_prefix,
                            model_id,
                            &target_prefix,
                            cwd,
                            raw_messages,
                            Some(err.reason),
                        )
                        .await;
                }
            }
        }

        // No canonical stack: legacy/MVP mode where export/import handoff is THE mechanism.
        // It is still surfaced as a fallback (§2288: handoff must be visible in UX).
        self.handoff_switch(
            trogon_session_id,
            current_model,
            current_prefix,
            model_id,
            &target_prefix,
            cwd,
            raw_messages,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn switch_via_session_kernel(
        &mut self,
        current_prefix: &str,
        trogon_session_id: &str,
        current_model: &str,
        target_prefix: &str,
        target_runner_session_id: Option<&str>,
        model_id: &str,
        cwd: &str,
        export_json: &str,
        stack: &crate::session_kernel::SessionKernelStack,
    ) -> Result<CanonicalSwitchOutcome, CanonicalSwitchFailure> {
        // Synthesize a CanonicalSwitchFailure (with its visible result) for failures that
        // have no rich orchestrator outcome — id/validation errors and post-attach
        // hydration failures. The blocked/confirmation gate path builds the RICH visible
        // result separately (it carries reasons/next_action from the orchestrator).
        let mk_fail = move |operation_id: &str, result: SwitchResult, reason: String| CanonicalSwitchFailure {
            operation_id: operation_id.to_string(),
            visible: failed_visible_result(
                trogon_session_id,
                current_model,
                current_prefix,
                model_id,
                target_prefix,
                result,
                reason.clone(),
            ),
            reason,
            result,
        };

        let session_id = SessionId::new(trogon_session_id)
            .map_err(|err| mk_fail(trogon_session_id, SwitchResult::FailedRecoverable, err.to_string()))?;
        let operation_id = OperationId::new(format!("op_switch_{}", uuid::Uuid::now_v7()))
            .map_err(|err| mk_fail(trogon_session_id, SwitchResult::FailedRecoverable, err.to_string()))?;
        let idempotency_key = IdempotencyKey::new(format!("idem_{}", uuid::Uuid::now_v7()))
            .map_err(|err| mk_fail(operation_id.as_str(), SwitchResult::FailedRecoverable, err.to_string()))?;

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

        let request = SwitchModelRequest {
            session_id: session_id.clone(),
            target_model: model_id.to_string(),
            reason: ModelSwitchReason::UserRequested,
            user_confirmed: true,
            force: false,
            force_acknowledged_losses: Vec::new(),
            operation_id: operation_id.clone(),
            correlation_id: format!("corr_{}", uuid::Uuid::now_v7()),
            idempotency_key,
        };

        // Always provide the REAL destination-checkpoint runner over NATS; the orchestrator
        // decides per-switch whether to actually invoke it (real checkpoint) or echo the
        // Context Twin, per the configured checkpoint strictness and the switch's risk
        // (§ Checkpoint strictness §1986/§2280: "switches con tools, artifacts, terminal o
        // cambios de proveedor deben usar checkpoint real contra destino"). When the policy
        // resolves to echo, the transport stays dormant (never asked).
        let transport = SessionAckTransport::new(
            self.nats.clone(),
            target_prefix.to_string(),
            model_id.to_string(),
            std::path::PathBuf::from(cwd),
        );
        let outcome = Box::pin(self.run_orchestrated_switch(
            stack,
            Arc::new(JsonAcknowledgementRunner::new(transport)),
            request,
        ))
        .await;
        // Normalize ONCE into the structured SwitchResult before unwrapping, so every
        // return path carries it (§ Trabajo restante: "emitir SwitchResult estructurado").
        let result_kind = classify_switch_result(&outcome);
        // Build the single durable visible result from the orchestrator outcome BEFORE it
        // is consumed (§ Contrato de resultado visible, §2084): degradations, lost
        // capabilities, checkpoint status and next_action all derive from it here.
        let visible_ctx = VisibleResultContext {
            session_id: trogon_session_id,
            from_model: current_model,
            from_runner: current_prefix,
            to_model: model_id,
            to_runner: target_prefix,
            fallback_used: false,
            fallback_reason: None,
        };
        let visible = build_visible_result(&visible_ctx, &outcome);

        let switch_result = outcome.map_err(|err| CanonicalSwitchFailure {
            operation_id: operation_id.as_str().to_string(),
            reason: err.to_string(),
            result: result_kind,
            visible: visible.clone(),
        })?;

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
                    visible,
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
                    visible,
                });
            }
        };

        let portable = {
            let legacy = legacy_portable_session_config(&self.nats, trogon_session_id).await;
            let snapshot = stack
                .kernel
                .load_snapshot(&session_id)
                .await
                // Canonical switch committed; only post-switch hydration failed, so the
                // session stays consistent and the attempt is retryable.
                .map_err(|err| mk_fail(operation_id.as_str(), SwitchResult::FailedRecoverable, err.to_string()))?;
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

        // Hydrate the destination runner from the CANONICAL conversation (per-message role
        // + full tool calls) rather than from the role-less, prompt-shaped projection
        // blocks. The canonical session is the no-lossy source of truth (cambio-modelo.md
        // §894 "el runner destino debe hidratarse desde la sesión Trogonai"); reconstructing
        // messages from projection blocks dropped tool calls, mislabeled turn roles, and
        // injected prompt scaffolding (system rules / context twin) as fake messages. The
        // projection stays for the checkpoint prompt and degradation metadata; the
        // destination runner textualizes structured tool blocks on import when text-only.
        let conversation = stack
            .kernel
            .load_snapshot(&session_id)
            .await
            .ok()
            .flatten()
            .and_then(|snapshot| snapshot.state.into_option())
            .map(|state| state.conversation)
            .unwrap_or_default();
        let import_messages = messages_json_for_runner_hydration(&PromptProjection::default(), &conversation)
            .map_err(|err| mk_fail(operation_id.as_str(), SwitchResult::FailedRecoverable, err.to_string()))?;

        let new_session_id = if let Some(target_runner_session_id) = target_runner_session_id {
            self.hydrate_existing_runner_session(
                target_prefix,
                target_runner_session_id,
                &import_messages,
                model_id,
                &portable,
            )
            .await
            .map_err(|reason| mk_fail(operation_id.as_str(), SwitchResult::FailedRecoverable, reason))?;
            target_runner_session_id.to_string()
        } else {
            self.open_and_hydrate_runner_session(target_prefix, cwd, &import_messages, model_id, &portable)
                .await
                .map_err(|reason| mk_fail(operation_id.as_str(), SwitchResult::FailedRecoverable, reason))?
        };

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
            visible,
        })
    }

    /// Build a `SwitchOrchestrator` with the given continuity-checkpoint runner and run
    /// the switch. Generic over the runner type so the checkpoint binding (Context-Twin
    /// echo vs real target-model query) is selected without dynamic dispatch.
    async fn run_orchestrated_switch<R: RunnerAcknowledgement + 'static>(
        &self,
        stack: &crate::session_kernel::SessionKernelStack,
        runner: Arc<R>,
        request: SwitchModelRequest,
    ) -> Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> {
        let orchestrator = SwitchOrchestrator::new(
            stack.kernel.clone(),
            stack.snapshots.clone(),
            stack.runner_bindings.clone(),
            stack.twin_store.clone(),
            self.registry.clone(),
            runner,
            stack.switching_config.clone(),
            stack.capability_config.clone(),
            stack.projection_config.clone(),
            stack.certification.clone(),
        );
        // Heap-pin the orchestrator future so the large canonical-switch state does not
        // inline into this caller's frame. The canonical path nests several async layers;
        // on a 2 MB worker/test stack an inlined chain overflows in debug builds. See the
        // matching note in `SwitchOrchestrator::switch_model`.
        Box::pin(orchestrator.switch_model(request)).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn handoff_switch(
        &mut self,
        current_session_id: &str,
        current_model: &str,
        current_prefix: &str,
        target_model: &str,
        target_prefix: &str,
        cwd: &str,
        raw_messages: &str,
        fallback_reason: Option<String>,
    ) -> Result<SwitchSurface, String> {
        if raw_messages.trim() == "null" || raw_messages.trim().is_empty() {
            return Err("session/export returned null — cannot import into new session".into());
        }
        let new_session_id = self
            .hydrate_new_runner_session(target_prefix, cwd, raw_messages)
            .await?;
        Ok(SwitchSurface {
            new_prefix: target_prefix.to_string(),
            new_session_id,
            // §2074/§2288/§2319: a handoff is surfaced as a fallback (degraded), never as a
            // clean canonical `switched`, so no surface fakes full continuity.
            visible: handoff_visible_result(
                current_session_id,
                current_model,
                current_prefix,
                target_model,
                target_prefix,
                fallback_reason,
            ),
        })
    }

    /// Open a fresh session on `prefix` and import `messages_json` into it, returning the
    /// new runner session id. Reused by handoff fallback and by event-log-primary resume
    /// (Fase 11: "el runner destino debe hidratarse desde el Session Kernel").
    async fn hydrate_new_runner_session(
        &mut self,
        prefix: &str,
        cwd: &str,
        messages_json: &str,
    ) -> Result<String, String> {
        self.ensure_bridge(prefix)?;
        let new_session_id = {
            let (bridge, _) = self.bridges.get(prefix).unwrap();
            bridge
                .new_session(NewSessionRequest::new(cwd))
                .await
                .map_err(|e| e.to_string())?
                .session_id
                .to_string()
        };

        let import_params = serde_json::value::RawValue::from_string(format!(
            r#"{{"sessionId":"{new_session_id}","messages":{messages_json}}}"#
        ))
        .map_err(|e| e.to_string())?;
        {
            let (bridge, _) = self.bridges.get(prefix).unwrap();
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

        Ok(new_session_id)
    }

    async fn hydrate_existing_runner_session(
        &mut self,
        target_prefix: &str,
        target_session_id: &str,
        import_messages_json: &str,
        model_id: &str,
        portable: &trogonai_switching::PortableRunnerConfig,
    ) -> Result<(), String> {
        self.ensure_bridge(target_prefix)?;
        let import_params = serde_json::value::RawValue::from_string(format!(
            r#"{{"sessionId":"{target_session_id}","messages":{import_messages_json}}}"#
        ))
        .map_err(|e| e.to_string())?;
        let (bridge, _) = self.bridges.get(target_prefix).unwrap();
        bridge
            .ext_method(ExtRequest::new("session/import", import_params.into()))
            .await
            .map_err(|err| err.to_string())?;
        apply_portable_runner_config(bridge, target_session_id, model_id, portable).await
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
        current_model: &'a str,
        model_id: &'a str,
        cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<SwitchSurface, String>> + 'a {
        CrossRunnerSwitcher::switch_model(self, current_prefix, current_session_id, current_model, model_id, cwd)
    }

    fn resume_event_primary<'a>(
        &'a mut self,
        prefix: &'a str,
        session_id: &'a str,
        cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<Option<String>, String>> + 'a {
        self.resume_event_primary_inner(prefix, session_id, cwd)
    }

    fn mark_session_event_primary<'a>(
        &'a self,
        session_id: &'a str,
    ) -> impl std::future::Future<Output = Result<(), String>> + 'a {
        self.mark_session_event_primary_inner(session_id)
    }

    fn certify_model<'a>(
        &'a mut self,
        prefix: &'a str,
        model: &'a str,
        cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<String, String>> + 'a {
        self.certify_model_label(prefix, model, cwd)
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
            current_prefix: &'a str,
            current_session_id: &'a str,
            current_model: &'a str,
            model_id: &'a str,
            _cwd: &'a str,
        ) -> impl std::future::Future<Output = Result<super::SwitchSurface, String>> + 'a {
            let result = self.result.clone();
            async move {
                let (new_prefix, new_session_id) = result?;
                // Same runner → clean in-place switch; different runner → handoff fallback
                // (the mock has no canonical kernel), matching the real surface contract.
                let visible = if new_prefix == current_prefix {
                    trogonai_switching::same_runner_visible_result(
                        current_session_id,
                        current_model,
                        model_id,
                        current_prefix,
                    )
                } else {
                    trogonai_switching::handoff_visible_result(
                        current_session_id,
                        current_model,
                        current_prefix,
                        model_id,
                        &new_prefix,
                        None,
                    )
                };
                Ok(super::SwitchSurface {
                    new_prefix,
                    new_session_id,
                    visible,
                })
            }
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
    /// The single durable visible result every surface renders (§2084).
    visible: SwitchVisibleResult,
}

struct CanonicalSwitchFailure {
    operation_id: String,
    reason: String,
    /// Structured outcome so the caller/UX derive behavior from the result, not text
    /// (§ "logs y metricas deben derivarse del resultado estructurado, no de parsear texto").
    result: SwitchResult,
    /// The visible result for this failure (carries reasons/next_action for blocked
    /// and confirmation-required, so the surface never has to parse the reason text).
    visible: SwitchVisibleResult,
}

/// Format the user-facing message for a blocked / confirmation-required switch from its
/// structured visible result (§2075/§2076: the surface must show the next_action and the
/// degradations/missing capabilities). Derived from the contract, not parsed from text.
fn blocked_surface_message(visible: &SwitchVisibleResult) -> String {
    let label = switch_result_label(visible.result.as_known().unwrap_or(SwitchResult::Blocked));
    let mut msg = format!("switch {label}");
    if !visible.degradations.is_empty() {
        msg.push_str(": ");
        msg.push_str(&visible.degradations.join("; "));
    }
    if let Some(next) = &visible.next_action {
        msg.push_str(&format!(" (next: {next})"));
    }
    msg
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
    fn visible_result_derives_user_text_from_structured_result() {
        // A terminal failure carries the structured result and exposes the concrete cause
        // as next_action (§ Reglas del contrato visible). The blocked message DERIVES from
        // the visible result, never from parsing free text (§2031).
        let terminal = failed_visible_result(
            "sess_1",
            "from-model",
            "acp.src",
            "ghost",
            "acp.dst",
            SwitchResult::FailedTerminal,
            "target model not found".to_string(),
        );
        assert_eq!(terminal.result.as_known(), Some(SwitchResult::FailedTerminal));
        assert_eq!(terminal.next_action.as_deref(), Some("target model not found"));

        let blocked = failed_visible_result(
            "sess_1",
            "from-model",
            "acp.src",
            "ghost",
            "acp.dst",
            SwitchResult::Blocked,
            "blocked".to_string(),
        );
        assert!(blocked_surface_message(&blocked).contains("blocked"));
    }

    #[test]
    fn switch_result_label_is_stable_for_every_state() {
        assert_eq!(switch_result_label(SwitchResult::Switched), "switched");
        assert_eq!(switch_result_label(SwitchResult::Blocked), "blocked");
        assert_eq!(
            switch_result_label(SwitchResult::RequiresConfirmation),
            "requires_confirmation"
        );
        assert_eq!(switch_result_label(SwitchResult::Degraded), "degraded");
        assert_eq!(switch_result_label(SwitchResult::Repaired), "repaired");
        assert_eq!(switch_result_label(SwitchResult::RolledBack), "rolled_back");
        assert_eq!(
            switch_result_label(SwitchResult::FailedRecoverable),
            "failed_recoverable"
        );
        assert_eq!(switch_result_label(SwitchResult::FailedTerminal), "failed_terminal");
    }

    #[tokio::test]
    async fn same_runner_returns_unchanged() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.test")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher
            .switch_model("acp.test", "session-abc", "sonnet", "gpt-4", "/workspace")
            .await;
        let surface = result.expect("same-runner switch must succeed");
        assert_eq!(surface.new_prefix, "acp.test");
        assert_eq!(surface.new_session_id, "session-abc");
        assert!(
            !surface.visible.runner_changed,
            "same runner must not report a runner change"
        );
    }

    #[tokio::test]
    async fn model_not_found_returns_error() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher
            .switch_model("acp.current", "session-1", "sonnet", "unknown-model", "/ws")
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
        let result = switcher
            .switch_model("acp.other", "session-1", "sonnet", "gpt-4", "/ws")
            .await;
        assert_eq!(result, Err("missing acp_prefix in registry metadata".to_string()));
    }

    #[tokio::test]
    async fn invalid_target_prefix_returns_error() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());
        // "acp..bad" has consecutive dots — AcpPrefix::new rejects it
        registry.register(&cap_with_prefix("gpt-4", "acp..bad")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher
            .switch_model("acp.current", "session-1", "sonnet", "gpt-4", "/ws")
            .await;
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
            .switch_model("acp.src", "old-session-id", "sonnet", "gpt-4", "/workspace")
            .await;

        let surface = result.expect("cross-runner migration must succeed");
        assert_eq!(surface.new_prefix, "acp.tgt");
        assert_eq!(surface.new_session_id, "migrated-session");
        // No canonical kernel here: the switch is a legacy handoff, surfaced as such.
        assert!(
            surface.visible.fallback_used,
            "legacy handoff must surface fallback_used=true"
        );
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
        let result = switcher
            .switch_model("acp.src", "session-1", "sonnet", "gpt-4", "/ws")
            .await;
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
        let result = switcher
            .switch_model("acp.src", "session-1", "sonnet", "gpt-4", "/ws")
            .await;
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
        let result = switcher
            .switch_model("acp.src", "session-1", "sonnet", "gpt-4", "/ws")
            .await;
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

        let r1 = switcher
            .switch_model("acp.src", "old-1", "sonnet", "gpt-4", "/ws")
            .await
            .unwrap();
        assert_eq!(r1.new_prefix, "acp.tgt");
        assert_eq!(r1.new_session_id, "s1");

        let r2 = switcher
            .switch_model("acp.src", "old-2", "sonnet", "gpt-4", "/ws")
            .await
            .unwrap();
        assert_eq!(r2.new_prefix, "acp.tgt");
        assert_eq!(r2.new_session_id, "s2");
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
            .switch_model("acp.src", "session-1", "sonnet", "gpt-4", "/workspace/myproject")
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
            .switch_model("acp.src", "src-session", "sonnet", "gpt-4", "/ws")
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
            .switch_model("acp.src", "src-session", "sonnet", "gpt-4", "/ws")
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
