use std::sync::Arc;

use buffa::{EnumValue, MessageField};
use time::OffsetDateTime;
use trogon_registry::{Registry, RegistryStore};
use trogonai_capabilities::{
    CapabilityConfig, ProviderCertificationMatrix, create_switch_adaptation_plan, resolve_model_capabilities,
};
use trogonai_session_contracts::{
    __buffa::oneof::content_block::Kind as BlockKind, Actor, ActorType, CanonicalMessage, CapabilityAdaptationAction,
    ContentBlock, ContinuityCheckpointStatus, ModelSwitchReason, PromptProjection, SessionId, SessionSnapshot,
    SessionSnapshotState, SwitchAdaptation, SwitchAdaptationPlan, SwitchResult, SwitchSafetyDecision, SwitchSafetyStatus,
};
use trogonai_session_kernel::{
    EventLogBackend, SessionKernel, SessionLeaseFactory, SessionMutatingOperation, SnapshotStore,
};
use trogonai_session_projection::{
    ContextTwinStore, ContextTwinUpdateContext, DefaultPromptCompiler, ProjectionConfig, ProjectionInput,
    PromptCompiler, update_context_twin,
};

use crate::checkpoint::{
    RunnerAcknowledgement, is_high_risk_switch, requires_continuity_checkpoint, run_continuity_checkpoint,
};
use crate::config::SwitchingConfig;
use crate::error::SwitchingError;
use crate::event::{
    SwitchFlowContext, compactor_model_preserved_event, compactor_model_unavailable_event,
    continuity_checkpoint_completed_event, continuity_checkpoint_started_event, fallback_to_default_compactor_event,
    force_switch_completed_event, force_switch_confirmed_event, force_switch_rejected_event,
    force_switch_requested_event, model_switched_event, runner_failed_event, switch_adaptation_plan_created_event,
    switch_outcome_recorded_event, switch_safety_evaluated_event,
};
use crate::force::{ForceSwitchOutcome, ForceSwitchRequest, evaluate_force_switch};
use crate::runner::{RunnerBindingStore, attach_runner, detach_runner, invalidate_nonportable_runner_state};
use crate::safety::{SwitchSafetyInput, evaluate_switch_safety};
use crate::state::SwitchState;
use crate::telemetry;
use crate::visible_result::{VisibleResultContext, build_visible_result};

/// User-initiated model switch request.
#[derive(Debug, Clone)]
pub struct SwitchModelRequest {
    pub session_id: SessionId,
    pub target_model: String,
    pub reason: ModelSwitchReason,
    pub user_confirmed: bool,
    pub force: bool,
    pub force_acknowledged_losses: Vec<String>,
    pub operation_id: trogonai_session_contracts::OperationId,
    pub correlation_id: String,
    pub idempotency_key: trogonai_session_contracts::IdempotencyKey,
}

/// Completed switch result returned to callers.
#[derive(Debug, Clone)]
pub struct SwitchCompletion {
    pub state: SwitchState,
    pub from_model: String,
    pub from_runner: String,
    pub to_model: String,
    pub to_runner: String,
    pub safety: SwitchSafetyDecision,
    pub adaptation_plan: trogonai_session_contracts::SwitchAdaptationPlan,
    pub projection: PromptProjection,
    pub checkpoint: Option<trogonai_session_contracts::ContinuityCheckpointResult>,
    pub events_appended: usize,
}

/// Early exit when safety gate blocks or requires confirmation.
#[derive(Debug, Clone)]
pub enum SwitchGateOutcome {
    Blocked(SwitchSafetyDecision),
    ConfirmationRequired(SwitchSafetyDecision),
}

/// Classify a switch outcome into the formal, shared [`SwitchResult`] contract
/// (§ "Contrato formal de resultado del switch"). Every switch normalizes to exactly
/// one of the 8 durable states, so events, metrics and UX/API derive from a contract
/// rather than free-text logs.
pub fn classify_switch_result(
    outcome: &Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError>,
) -> SwitchResult {
    match outcome {
        Ok(Ok(completion)) => {
            let repaired = completion
                .checkpoint
                .as_ref()
                .and_then(|checkpoint| checkpoint.status.as_known())
                == Some(ContinuityCheckpointStatus::Repaired);
            if repaired {
                SwitchResult::Repaired
            } else if switch_was_degraded(completion) {
                SwitchResult::Degraded
            } else {
                SwitchResult::Switched
            }
        }
        Ok(Err(SwitchGateOutcome::Blocked(_))) => SwitchResult::Blocked,
        Ok(Err(SwitchGateOutcome::ConfirmationRequired(_))) => SwitchResult::RequiresConfirmation,
        Err(SwitchingError::SwitchRolledBack { .. }) => SwitchResult::RolledBack,
        Err(err) if switch_error_is_terminal(err) => SwitchResult::FailedTerminal,
        Err(_) => SwitchResult::FailedRecoverable,
    }
}

/// `degraded` (§1976): the switch executed but with visible, recorded degradations —
/// adaptation-plan warnings or a "with warning" safety decision.
fn switch_was_degraded(completion: &SwitchCompletion) -> bool {
    !completion.adaptation_plan.warnings.is_empty()
        || completion.safety.status.as_known() == Some(SwitchSafetyStatus::AllowedWithWarning)
        || !completion.safety.reasons.is_empty()
}

/// `failed_terminal` (§1980, §2023): runner inexistente, schema incompatible, config
/// faltante, transición inválida o force-switch rechazado — todo lo que requiere
/// intervención/cambio de config antes de reintentar. Lo demás es `failed_recoverable`
/// (la sesión canónica sigue consistente y se puede reintentar).
fn switch_error_is_terminal(err: &SwitchingError) -> bool {
    matches!(
        err,
        SwitchingError::TargetModelNotFound { .. }
            | SwitchingError::MissingField(_)
            | SwitchingError::ForceSwitchRejected { .. }
            | SwitchingError::InvalidSwitchTransition { .. }
            | SwitchingError::InvalidCancelTransition { .. }
            | SwitchingError::InvalidForceSwitchTransition { .. }
            | SwitchingError::Kernel(
                trogonai_session_kernel::SessionKernelError::ContractValidation(_)
            )
    )
}

/// §598-604 / §1897 self-healing: the repair adaptation plan used to RECOMPILE the
/// projection with more context when a continuity checkpoint comes back `Repaired`. It
/// forces the Context-Twin-plus-recent-turns projection (the §478 `context_window`
/// adaptation) so the destination model receives the session state prominently instead of
/// the borderline projection that triggered the repair. Pure + deterministic.
fn context_twin_repair_plan(base: &SwitchAdaptationPlan) -> SwitchAdaptationPlan {
    let mut plan = base.clone();
    let repair_action = EnumValue::Known(CapabilityAdaptationAction::UseContextTwinPlusRecentTurns);
    if let Some(existing) = plan.adaptations.iter_mut().find(|a| a.capability == "context_window") {
        existing.action = repair_action;
    } else {
        plan.adaptations.push(SwitchAdaptation {
            capability: "context_window".to_string(),
            action: repair_action,
            ..SwitchAdaptation::default()
        });
    }
    let repair_warning = "context repaired: recompiled with Context Twin + recent turns".to_string();
    if !plan.warnings.contains(&repair_warning) {
        plan.warnings.push(repair_warning);
    }
    plan
}

/// Orchestrates the 22-step canonical model switch flow.
pub struct SwitchOrchestrator<E, SnapKv, LeaseF, BindingKv, TwinKv, RegStore, R>
where
    RegStore: RegistryStore,
{
    kernel: SessionKernel<E, SnapKv, LeaseF>,
    snapshots: SnapshotStore<SnapKv>,
    runner_bindings: RunnerBindingStore<BindingKv>,
    twin_store: ContextTwinStore<TwinKv>,
    registry: Registry<RegStore>,
    runner: Arc<R>,
    switching_config: SwitchingConfig,
    capability_config: CapabilityConfig,
    projection_config: ProjectionConfig,
    certification: ProviderCertificationMatrix,
    compiler: DefaultPromptCompiler,
}

impl<E, SnapKv, LeaseF, BindingKv, TwinKv, RegStore, R>
    SwitchOrchestrator<E, SnapKv, LeaseF, BindingKv, TwinKv, RegStore, R>
where
    E: EventLogBackend,
    SnapKv: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    LeaseF: SessionLeaseFactory + Clone + 'static,
    BindingKv: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    TwinKv: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    RegStore: RegistryStore,
    R: RunnerAcknowledgement,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kernel: SessionKernel<E, SnapKv, LeaseF>,
        snapshots: SnapshotStore<SnapKv>,
        runner_bindings: RunnerBindingStore<BindingKv>,
        twin_store: ContextTwinStore<TwinKv>,
        registry: Registry<RegStore>,
        runner: Arc<R>,
        switching_config: SwitchingConfig,
        capability_config: CapabilityConfig,
        projection_config: ProjectionConfig,
        certification: ProviderCertificationMatrix,
    ) -> Self {
        Self {
            kernel,
            snapshots,
            runner_bindings,
            twin_store,
            registry,
            runner,
            switching_config,
            capability_config,
            projection_config,
            certification,
            compiler: DefaultPromptCompiler,
        }
    }

    /// Execute the full canonical model switch flow under Session Lease.
    pub async fn switch_model(
        &self,
        request: SwitchModelRequest,
    ) -> Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> {
        let mut switch_state = SwitchState::Requested;
        let mut events_appended = 0_usize;
        let now = OffsetDateTime::now_utc();
        let created_at = timestamp(now);

        let flow = SwitchFlowContext {
            session_id: request.session_id.clone(),
            operation_id: request.operation_id.clone(),
            correlation_id: request.correlation_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            causation_id: None,
            actor: kernel_actor(),
            created_at: created_at.clone(),
        };

        // Hold the lease AND a renewal heartbeat for the whole switch so a long
        // operation never lets it expire mid-flight ("heartbeat/renew mientras dura
        // la operacion"). Both are kept in scope until the function returns.
        let (lease_guard, lease_renewal) = match self
            .kernel
            .acquire_session_lease_renewing(&request.session_id, SessionMutatingOperation::SwitchModel)
            .await
        {
            Ok(held) => held,
            Err(trogonai_session_kernel::SessionKernelError::SessionBusy { session_id, .. }) => {
                return Err(SwitchingError::SessionBusy { session_id });
            }
            Err(err) => return Err(err.into()),
        };
        let log_session_id = request.session_id.as_str().to_string();
        let log_operation_id = request.operation_id.as_str().to_string();
        // Captured before the `async move` consumes `request`/`flow`, so the durable
        // outcome record can be appended after the flow completes (still under lease).
        let target_model = request.target_model.clone();
        let result_flow = flow.clone();

        // Run the whole switch under the lease, then release it EXPLICITLY (Session
        // Lease flow step 22: acquire -> mutation -> append -> materialize -> release).
        // The `async move` block captures every `?` and early return so the explicit
        // release below always runs; the lease TTL stays only as the crash fallback.
        //
        // `Box::pin` keeps this large per-switch future on the heap instead of inlining
        // it into the caller's future. The canonical switch path nests many `async fn`
        // layers (cross_runner -> run_orchestrated_switch -> here), and in debug builds an
        // inlined chain of large futures produces a single oversized poll frame that
        // overflows the current_thread runtime's stack. Boxing the biggest segment breaks
        // that chain. Regression-covered by the real-NATS canonical switch e2e test.
        let switch_outcome: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> = Box::pin(async move {
            switch_state = switch_state.transition_to(SwitchState::LeaseAcquired)?;

            let snapshot = self.load_or_materialize(&request.session_id).await?;
            let last_applied_seq = snapshot.last_applied_seq;
            let mut session_state = snapshot
                .state
                .into_option()
                .ok_or(SwitchingError::MissingField("snapshot.state"))?;

            let from_model = session_state
                .config
                .as_option()
                .and_then(|config| config.model.clone())
                .ok_or(SwitchingError::MissingField("config.model"))?;
            let from_runner = session_state
                .config
                .as_option()
                .and_then(|config| config.runner.clone())
                .unwrap_or_default();

            let target_capabilities =
                resolve_model_capabilities(&self.registry, &request.target_model, now, &self.capability_config).await?;
            let target_runner = target_capabilities.runner_id.clone();

            let (context_twin, _twin_event) = update_context_twin(
                &self.kernel,
                &self.twin_store,
                ContextTwinUpdateContext {
                    session_id: request.session_id.clone(),
                    operation_id: request.operation_id.clone(),
                    correlation_id: request.correlation_id.clone(),
                    idempotency_key: trogonai_session_contracts::IdempotencyKey::new(format!(
                        "{}_twin",
                        request.idempotency_key.as_str()
                    ))
                    .map_err(|err| {
                        SwitchingError::Kernel(trogonai_session_kernel::SessionKernelError::ContractValidation(
                            trogonai_session_contracts::ContractValidationError::InvalidIdempotencyKey(err),
                        ))
                    })?,
                    causation_id: None,
                    actor: kernel_actor(),
                    updated_at: created_at.clone(),
                },
            )
            .await?;
            events_appended += 1;
            session_state.context_twin = MessageField::some(context_twin.clone());

            let adaptation_plan = create_switch_adaptation_plan(
                &self.registry,
                &session_state,
                &request.target_model,
                now,
                &self.capability_config,
            )
            .await?;
            switch_state = switch_state.transition_to(SwitchState::AdaptationPlanned)?;

            let adapt_event = switch_adaptation_plan_created_event(&flow, &adaptation_plan);
            self.kernel.append_event(adapt_event).await?;
            events_appended += 1;
            session_state.switch_adaptation_plan = MessageField::some(adaptation_plan.clone());

            // Record compactor_model preservation/degradation (§ Token Budget Policy).
            // The actual fallback is only recorded later, once the switch proceeds past
            // the safety gate, so degradation of an explicit user preference stays explicit.
            let compactor_model_unavailable = if let Some(compactor_model) = session_state
                .config
                .as_option()
                .and_then(|config| config.compactor_model.clone())
            {
                if target_capabilities.schema.compaction_supported {
                    self.kernel
                        .append_event(compactor_model_preserved_event(&flow, &compactor_model))
                        .await?;
                    events_appended += 1;
                    None
                } else {
                    self.kernel
                        .append_event(compactor_model_unavailable_event(
                            &flow,
                            &compactor_model,
                            &request.target_model,
                        ))
                        .await?;
                    events_appended += 1;
                    Some(compactor_model)
                }
            } else {
                None
            };

            let safety = evaluate_switch_safety(&SwitchSafetyInput {
                session: &session_state,
                target_model: &request.target_model,
                target_runner: &target_runner,
                target_capabilities: &target_capabilities,
                adaptation_plan: Some(&adaptation_plan),
                certification: &self.certification,
                config: &self.switching_config,
                force: request.force,
                user_confirmed: request.user_confirmed,
                last_applied_seq,
            });
            switch_state = switch_state.transition_to(SwitchState::SafetyEvaluated)?;

            let safety_event = switch_safety_evaluated_event(&flow, &safety);
            self.kernel.append_event(safety_event).await?;
            events_appended += 1;
            session_state.switch_safety = MessageField::some(safety.clone());

            // § Continuity Metrics and Evals (§1165): a degraded-but-allowed switch is a
            // continuity signal worth measuring.
            if safety.status.as_known() == Some(SwitchSafetyStatus::AllowedWithWarning) {
                telemetry::metrics::record_switch_allowed_with_warning(
                    request.session_id.as_str(),
                    request.operation_id.as_str(),
                    &request.target_model,
                );
            }

            if request.force {
                self.kernel
                    .append_event(force_switch_requested_event(&flow, &request.force_acknowledged_losses))
                    .await?;
                events_appended += 1;
                let (outcome, _force_state) = evaluate_force_switch(
                    &ForceSwitchRequest {
                        confirmed: request.user_confirmed,
                        acknowledged_losses: request.force_acknowledged_losses.clone(),
                    },
                    &session_state,
                    &safety,
                )?;
                match outcome {
                    ForceSwitchOutcome::Completed {
                        invalidated,
                        pending_reconciliation,
                    } => {
                        self.kernel.append_event(force_switch_confirmed_event(&flow)).await?;
                        events_appended += 1;
                        self.kernel
                            .append_event(force_switch_completed_event(
                                &flow,
                                &invalidated,
                                &pending_reconciliation,
                            ))
                            .await?;
                        events_appended += 1;
                    }
                    ForceSwitchOutcome::Rejected(reason) => {
                        self.kernel
                            .append_event(force_switch_rejected_event(&flow, &reason))
                            .await?;
                        return Ok(Err(SwitchGateOutcome::ConfirmationRequired(safety)));
                    }
                }
            } else {
                match safety.status.as_known() {
                    Some(SwitchSafetyStatus::BlockedUntilSafe) => {
                        return Ok(Err(SwitchGateOutcome::Blocked(safety)));
                    }
                    Some(SwitchSafetyStatus::RequiresUserConfirmation) if !request.user_confirmed => {
                        return Ok(Err(SwitchGateOutcome::ConfirmationRequired(safety)));
                    }
                    _ => {}
                }
            }

            let switch_event = model_switched_event(
                &flow,
                &from_runner,
                &from_model,
                &target_runner,
                &request.target_model,
                request.reason,
            );
            self.kernel.append_event(switch_event).await?;
            events_appended += 1;
            switch_state = switch_state.transition_to(SwitchState::ModelSwitched)?;

            // The switch proceeded past the gate, so an unavailable compactor_model now
            // falls back to the default — record it explicitly (§ Token Budget Policy).
            if let Some(compactor_model) = &compactor_model_unavailable {
                self.kernel
                    .append_event(fallback_to_default_compactor_event(
                        &flow,
                        compactor_model,
                        &request.target_model,
                    ))
                    .await?;
                events_appended += 1;
            }

            if from_runner != target_runner {
                let binding_context = flow.runner_binding_context();
                let (detach_event, _) = detach_runner(
                    &self.runner_bindings,
                    &binding_context,
                    &from_runner,
                    &from_model,
                    "model_switch",
                    &mut session_state,
                )
                .await?;
                self.kernel.append_event(detach_event).await?;
                events_appended += 1;
                switch_state = switch_state.transition_to(SwitchState::RunnerDetached)?;

                self.persist_snapshot_with_nonportable_cleared(&request.session_id, &mut session_state)
                    .await?;
            }

            let capability_snapshot_id = Some(format!("cap_{}", uuid::Uuid::now_v7()));
            let (binding, attach_event, _) = attach_runner(
                &self.runner_bindings,
                &flow.runner_binding_context(),
                &target_runner,
                &request.target_model,
                capability_snapshot_id,
            )
            .await
            .inspect_err(|_err| {
                // § Continuity Metrics and Evals (§1165): a failed runner attach is a
                // switch-failure signal.
                telemetry::metrics::record_runner_attach_failure(
                    request.session_id.as_str(),
                    &target_runner,
                    &request.target_model,
                );
            })?;
            self.kernel.append_event(attach_event).await?;
            events_appended += 1;
            switch_state = switch_state.transition_to(SwitchState::RunnerAttached)?;
            session_state.active_runner_binding = MessageField::some(binding);

            let continuity_warnings: Vec<String> = safety
                .reasons
                .iter()
                .map(|reason| format!("{}: {}", reason.kind, reason.detail))
                .collect();

            let current_request = session_state
                .conversation
                .iter()
                .rev()
                .find(|message| message.role == "user")
                .cloned()
                .or_else(|| switch_continuity_request(&request.target_model, &created_at));

            ensure_projection_prerequisites(&mut session_state);

            let mut projection = self.compiler.compile(ProjectionInput {
                session_id: request.session_id.as_str().to_string(),
                model_id: request.target_model.clone(),
                snapshot: session_state.clone(),
                context_twin: context_twin.clone(),
                adaptation_plan: Some(adaptation_plan.clone()),
                capabilities: target_capabilities.clone(),
                token_budget: target_capabilities.schema.max_context_tokens,
                current_request: current_request.clone(),
                continuity_warnings: continuity_warnings.clone(),
                config: self.projection_config.clone(),
                created_at: Some(created_at.clone()),
                projection_id: Some(format!("proj_{}", uuid::Uuid::now_v7())),
            })?;
            switch_state = switch_state.transition_to(SwitchState::ProjectionCompiled)?;

            let mut checkpoint_result = None;
            if requires_continuity_checkpoint(
                &session_state,
                &from_runner,
                &target_runner,
                Some(&adaptation_plan),
                &self.switching_config,
            ) {
                let checkpoint_id = format!("checkpoint_{}", uuid::Uuid::now_v7());
                let started =
                    continuity_checkpoint_started_event(&flow, &checkpoint_id, &request.target_model, &target_runner);
                self.kernel.append_event(started).await?;
                events_appended += 1;

                // § Checkpoint strictness (§1986/§2280): escalate a high-risk switch to a
                // real destination checkpoint per the configured policy, even when the
                // global echo default is on.
                let high_risk = is_high_risk_switch(&session_state, &from_runner, &target_runner);
                let use_real = self.switching_config.requires_real_checkpoint(high_risk);
                let (result, _) = match run_continuity_checkpoint(
                    self.runner.as_ref(),
                    request.session_id.as_str(),
                    &request.target_model,
                    &context_twin,
                    &projection,
                    &self.switching_config,
                    use_real,
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(err @ SwitchingError::RunnerAcknowledgementFailed { .. }) => {
                        // Runner failed after runner_attached: record runner_failed (its
                        // materialization invalidates the active binding) and preserve
                        // session state (§ Failure Mode Policy). A low-confidence
                        // checkpoint is not a runner failure and propagates unchanged.
                        self.kernel
                            .append_event(runner_failed_event(
                                &flow.runner_binding_context(),
                                &target_runner,
                                &request.target_model,
                                &err.to_string(),
                            ))
                            .await?;
                        return Err(err);
                    }
                    Err(err @ SwitchingError::CheckpointFailed { .. }) => {
                        // § rolled_back: the continuity checkpoint — a stage AFTER
                        // runner_attached — failed. Restore the previous runner binding so
                        // the canonical session stays consistent on the prior model, then
                        // surface `rolled_back` (§2032; § Rollback Strategy; §604: a failed
                        // checkpoint may resolve by rollback). Detach the just-attached
                        // target and re-attach the source binding. The rollback events use a
                        // distinct `_rollback` idempotency key so they are not deduplicated
                        // against the forward detach/attach (whose keys derive from the same
                        // operation).
                        let mut rollback_flow = flow.clone();
                        rollback_flow.idempotency_key = trogonai_session_contracts::IdempotencyKey::new(format!(
                            "{}_rollback",
                            flow.idempotency_key.as_str()
                        ))
                        .map_err(|key_err| {
                            SwitchingError::Kernel(trogonai_session_kernel::SessionKernelError::ContractValidation(
                                trogonai_session_contracts::ContractValidationError::InvalidIdempotencyKey(key_err),
                            ))
                        })?;
                        let binding_context = rollback_flow.runner_binding_context();
                        let (rollback_detach, _) = detach_runner(
                            &self.runner_bindings,
                            &binding_context,
                            &target_runner,
                            &request.target_model,
                            "rollback_checkpoint_failed",
                            &mut session_state,
                        )
                        .await?;
                        self.kernel.append_event(rollback_detach).await?;
                        let (restored_binding, rollback_attach, _) =
                            attach_runner(&self.runner_bindings, &binding_context, &from_runner, &from_model, None)
                                .await?;
                        self.kernel.append_event(rollback_attach).await?;
                        session_state.active_runner_binding = MessageField::some(restored_binding);
                        self.persist_snapshot_with_nonportable_cleared(&request.session_id, &mut session_state)
                            .await?;
                        return Err(SwitchingError::SwitchRolledBack { detail: err.to_string() });
                    }
                    Err(err) => return Err(err),
                };
                // §598-604 / §1897 self-healing: a `Repaired` checkpoint means the
                // destination's understanding was borderline. Recompile the projection with
                // more context (Context Twin + recent turns) so the continuation actually
                // carries the repaired context — not just the recorded
                // `recompile_with_more_context` marker. Best-effort: if the recompile fails
                // we keep the original projection (the checkpoint already passed as repaired).
                if result.status.as_known() == Some(ContinuityCheckpointStatus::Repaired) {
                    let repair_plan = context_twin_repair_plan(&adaptation_plan);
                    if let Ok(repaired) = self.compiler.compile(ProjectionInput {
                        session_id: request.session_id.as_str().to_string(),
                        model_id: request.target_model.clone(),
                        snapshot: session_state.clone(),
                        context_twin: context_twin.clone(),
                        adaptation_plan: Some(repair_plan),
                        capabilities: target_capabilities.clone(),
                        token_budget: target_capabilities.schema.max_context_tokens,
                        current_request: current_request.clone(),
                        continuity_warnings: continuity_warnings.clone(),
                        config: self.projection_config.clone(),
                        created_at: Some(created_at.clone()),
                        projection_id: Some(format!("proj_repair_{}", uuid::Uuid::now_v7())),
                    }) {
                        projection = repaired;
                    }
                }

                let completed = continuity_checkpoint_completed_event(&flow, &result);
                self.kernel.append_event(completed).await?;
                events_appended += 1;
                session_state.continuity_checkpoint = MessageField::some(result.clone());
                checkpoint_result = Some(result);
                switch_state = switch_state.transition_to(SwitchState::CheckpointPassed)?;
            }

            let materialized = self.kernel.materialize_state(&request.session_id).await?;
            let _ = materialized;
            switch_state = switch_state.transition_to(SwitchState::Completed)?;

            telemetry::metrics::record_switch_completed(
                request.session_id.as_str(),
                request.operation_id.as_str(),
                &from_model,
                &request.target_model,
                &from_runner,
                &target_runner,
            );

            Ok(Ok(SwitchCompletion {
                state: switch_state,
                from_model,
                from_runner,
                to_model: request.target_model,
                to_runner: target_runner,
                safety,
                adaptation_plan,
                projection,
                checkpoint: checkpoint_result,
                events_appended,
            }))
        })
        .await;

        // Normalize the attempt to ONE visible result, persist it as a durable event,
        // and derive the metric from it (§ Contrato formal/visible de resultado del
        // switch: cada resultado mapea a evento durable y a métricas, no a texto libre).
        // Appended while the lease is still held (acquire -> ... -> append -> release).
        let switch_result = classify_switch_result(&switch_outcome);
        let (from_model, from_runner, to_model, to_runner) = match &switch_outcome {
            Ok(Ok(completion)) => (
                completion.from_model.clone(),
                completion.from_runner.clone(),
                completion.to_model.clone(),
                completion.to_runner.clone(),
            ),
            // Gate-stopped or errored before binding: source/target runner unresolved.
            _ => (String::new(), String::new(), target_model.clone(), String::new()),
        };
        let visible = build_visible_result(
            &VisibleResultContext {
                session_id: &log_session_id,
                from_model: &from_model,
                from_runner: &from_runner,
                to_model: &to_model,
                to_runner: &to_runner,
                fallback_used: false,
                fallback_reason: None,
            },
            &switch_outcome,
        );
        if let Err(err) = self
            .kernel
            .append_event(switch_outcome_recorded_event(&result_flow, visible))
            .await
        {
            tracing::warn!(session_id = %log_session_id, error = %err, "switch: outcome record append failed");
        }
        telemetry::metrics::record_switch_result(&log_session_id, &log_operation_id, switch_result);

        // Release the lease explicitly (Session Lease flow); TTL is only the crash
        // fallback. Non-fatal: on failure the lease still expires by TTL.
        if let Err(err) = self
            .kernel
            .release_session_lease_renewing(lease_guard, lease_renewal)
            .await
        {
            tracing::warn!(session_id = %log_session_id, error = %err, "switch: lease release failed");
        }

        switch_outcome
    }

    async fn load_or_materialize(&self, session_id: &SessionId) -> Result<SessionSnapshot, SwitchingError> {
        if let Some(snapshot) = self.kernel.load_snapshot(session_id).await? {
            return Ok(snapshot);
        }
        Ok(self.kernel.materialize_state(session_id).await?)
    }

    async fn persist_snapshot_with_nonportable_cleared(
        &self,
        session_id: &SessionId,
        session_state: &mut SessionSnapshotState,
    ) -> Result<(), SwitchingError> {
        invalidate_nonportable_runner_state(session_state);
        let mut snapshot = self.load_or_materialize(session_id).await?;
        if let Some(state) = snapshot.state.as_option_mut() {
            invalidate_nonportable_runner_state(state);
        } else {
            snapshot.state = MessageField::some(session_state.clone());
        }
        self.snapshots.save_snapshot(&snapshot).await?;
        Ok(())
    }
}

fn kernel_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "trogonai-switching".to_string(),
        ..Actor::default()
    }
}

fn timestamp(now: OffsetDateTime) -> buffa_types::google::protobuf::Timestamp {
    buffa_types::google::protobuf::Timestamp {
        seconds: now.unix_timestamp(),
        nanos: now.nanosecond() as i32,
        ..buffa_types::google::protobuf::Timestamp::default()
    }
}

fn switch_continuity_request(
    target_model: &str,
    created_at: &buffa_types::google::protobuf::Timestamp,
) -> Option<CanonicalMessage> {
    Some(CanonicalMessage {
        message_id: format!("msg_switch_{}", uuid::Uuid::now_v7()),
        role: "user".to_string(),
        content: vec![ContentBlock {
            kind: Some(BlockKind::Text(format!(
                "Continue this session using model {target_model}."
            ))),
            ..ContentBlock::default()
        }],
        created_at: MessageField::some(created_at.clone()),
        ..CanonicalMessage::default()
    })
}

fn ensure_projection_prerequisites(state: &mut SessionSnapshotState) {
    let config = state.config.get_or_insert_default();
    if config.system_prompt.is_none() {
        config.system_prompt = Some("Continue the active Trogonai session.".to_string());
    }
    if config.permission_rules_text.is_none() {
        config.permission_rules_text = Some("Apply portable session permission rules.".to_string());
    }
}

#[cfg(test)]
mod repair_tests {
    use super::*;

    #[test]
    fn context_twin_repair_plan_forces_context_twin_and_warns() {
        // §598-604/§1897: the repair plan must force the Context-Twin-plus-recent-turns
        // projection and record that context was repaired — even from an empty base plan.
        let repaired = context_twin_repair_plan(&SwitchAdaptationPlan::default());
        let ctx = repaired
            .adaptations
            .iter()
            .find(|a| a.capability == "context_window")
            .expect("context_window adaptation must be present");
        assert_eq!(ctx.action.as_known(), Some(CapabilityAdaptationAction::UseContextTwinPlusRecentTurns));
        assert!(repaired.warnings.iter().any(|w| w.contains("context repaired")));
    }

    #[test]
    fn context_twin_repair_plan_overrides_existing_context_window_action() {
        let base = SwitchAdaptationPlan {
            adaptations: vec![SwitchAdaptation {
                capability: "context_window".to_string(),
                action: EnumValue::Known(CapabilityAdaptationAction::Omit),
                ..SwitchAdaptation::default()
            }],
            ..SwitchAdaptationPlan::default()
        };
        let repaired = context_twin_repair_plan(&base);
        // Still exactly one context_window adaptation, now forced to the repair action.
        let ctx: Vec<_> = repaired.adaptations.iter().filter(|a| a.capability == "context_window").collect();
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx[0].action.as_known(), Some(CapabilityAdaptationAction::UseContextTwinPlusRecentTurns));
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;
    use trogonai_session_contracts::{
        ContinuityCheckpointResult, SwitchAdaptationPlan, SwitchSafetyReason,
    };

    fn safety(status: SwitchSafetyStatus, reasons: Vec<SwitchSafetyReason>) -> SwitchSafetyDecision {
        SwitchSafetyDecision {
            status: EnumValue::Known(status),
            reasons,
            ..SwitchSafetyDecision::default()
        }
    }

    fn completion(
        safety: SwitchSafetyDecision,
        warnings: Vec<String>,
        checkpoint: Option<ContinuityCheckpointStatus>,
    ) -> SwitchCompletion {
        SwitchCompletion {
            state: SwitchState::Completed,
            from_model: "claude-sonnet".to_string(),
            from_runner: "acp.claude".to_string(),
            to_model: "grok".to_string(),
            to_runner: "acp.grok".to_string(),
            safety,
            adaptation_plan: SwitchAdaptationPlan {
                warnings,
                ..SwitchAdaptationPlan::default()
            },
            projection: PromptProjection::default(),
            checkpoint: checkpoint.map(|status| ContinuityCheckpointResult {
                status: EnumValue::Known(status),
                ..ContinuityCheckpointResult::default()
            }),
            events_appended: 3,
        }
    }

    fn reason(kind: &str) -> SwitchSafetyReason {
        SwitchSafetyReason {
            kind: kind.to_string(),
            detail: "test".to_string(),
            ..SwitchSafetyReason::default()
        }
    }

    // §1954-1987: the 8 formal states must derive deterministically from the
    // Safety Gate outcome, runner/checkpoint result and error class.

    #[test]
    fn clean_switch_is_switched() {
        let c = completion(safety(SwitchSafetyStatus::Allowed, vec![]), vec![], None);
        let out = Ok(Ok(c));
        assert_eq!(classify_switch_result(&out), SwitchResult::Switched);
    }

    #[test]
    fn adaptation_warnings_are_degraded() {
        let c = completion(
            safety(SwitchSafetyStatus::Allowed, vec![]),
            vec!["dropped reasoning trace".to_string()],
            None,
        );
        assert_eq!(classify_switch_result(&Ok(Ok(c))), SwitchResult::Degraded);
    }

    #[test]
    fn allowed_with_warning_safety_is_degraded() {
        let c = completion(
            safety(SwitchSafetyStatus::AllowedWithWarning, vec![reason("capability_loss")]),
            vec![],
            None,
        );
        assert_eq!(classify_switch_result(&Ok(Ok(c))), SwitchResult::Degraded);
    }

    #[test]
    fn repaired_checkpoint_takes_precedence_over_degraded() {
        // A repaired projection plus warnings still classifies as Repaired: the
        // checkpoint repair is the more specific, higher-signal outcome.
        let c = completion(
            safety(SwitchSafetyStatus::Allowed, vec![]),
            vec!["minor drift".to_string()],
            Some(ContinuityCheckpointStatus::Repaired),
        );
        assert_eq!(classify_switch_result(&Ok(Ok(c))), SwitchResult::Repaired);
    }

    #[test]
    fn passed_checkpoint_is_not_repaired() {
        let c = completion(
            safety(SwitchSafetyStatus::Allowed, vec![]),
            vec![],
            Some(ContinuityCheckpointStatus::Passed),
        );
        assert_eq!(classify_switch_result(&Ok(Ok(c))), SwitchResult::Switched);
    }

    #[test]
    fn blocked_gate_is_blocked() {
        let out: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> =
            Ok(Err(SwitchGateOutcome::Blocked(safety(SwitchSafetyStatus::BlockedUntilSafe, vec![]))));
        assert_eq!(classify_switch_result(&out), SwitchResult::Blocked);
    }

    #[test]
    fn confirmation_gate_is_requires_confirmation() {
        let out: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> = Ok(Err(
            SwitchGateOutcome::ConfirmationRequired(safety(SwitchSafetyStatus::RequiresUserConfirmation, vec![])),
        ));
        assert_eq!(classify_switch_result(&out), SwitchResult::RequiresConfirmation);
    }

    #[test]
    fn target_model_not_found_is_failed_terminal() {
        let out: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> =
            Err(SwitchingError::TargetModelNotFound { model_id: "ghost".to_string() });
        assert_eq!(classify_switch_result(&out), SwitchResult::FailedTerminal);
    }

    #[test]
    fn session_busy_is_failed_recoverable() {
        let out: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> = Err(
            SwitchingError::SessionBusy { session_id: SessionId::new("sess_test").expect("valid id") },
        );
        assert_eq!(classify_switch_result(&out), SwitchResult::FailedRecoverable);
    }
}
