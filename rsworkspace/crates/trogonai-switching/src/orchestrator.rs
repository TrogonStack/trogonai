use std::sync::Arc;

use buffa::{EnumValue, MessageField};
use time::OffsetDateTime;
use trogon_registry::{Registry, RegistryStore};
use trogonai_capabilities::{
    CapabilityConfig, ProviderCertificationMatrix, create_switch_adaptation_plan,
    resolve_model_capabilities,
};
use trogonai_session_contracts::{
    Actor, ActorType, CanonicalMessage, ContentBlock, ModelSwitchReason, PromptProjection, SessionId,
    SessionSnapshot, SessionSnapshotState, SwitchSafetyDecision, SwitchSafetyStatus,
    __buffa::oneof::content_block::Kind as BlockKind,
};
use trogonai_session_kernel::{
    EventLogBackend, SessionKernel, SessionLeaseFactory, SessionMutatingOperation, SnapshotStore,
};
use trogonai_session_projection::{
    ContextTwinStore, DefaultPromptCompiler, ProjectionConfig, ProjectionInput, PromptCompiler,
    update_context_twin, ContextTwinUpdateContext,
};

use crate::checkpoint::{requires_continuity_checkpoint, run_continuity_checkpoint, RunnerAcknowledgement};
use crate::config::SwitchingConfig;
use crate::error::SwitchingError;
use crate::event::{
    compactor_model_preserved_event, compactor_model_unavailable_event,
    continuity_checkpoint_completed_event, continuity_checkpoint_started_event,
    fallback_to_default_compactor_event, force_switch_completed_event, force_switch_confirmed_event,
    force_switch_rejected_event, force_switch_requested_event, model_switched_event,
    runner_failed_event, switch_adaptation_plan_created_event, switch_safety_evaluated_event,
    SwitchFlowContext,
};
use crate::force::{evaluate_force_switch, ForceSwitchOutcome, ForceSwitchRequest};
use crate::runner::{attach_runner, detach_runner, invalidate_nonportable_runner_state, RunnerBindingStore};
use crate::safety::{evaluate_switch_safety, SwitchSafetyInput};
use crate::state::SwitchState;
use crate::telemetry;

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
pub struct SwitchResult {
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

impl<E, SnapKv, LeaseF, BindingKv, TwinKv, RegStore, R> SwitchOrchestrator<E, SnapKv, LeaseF, BindingKv, TwinKv, RegStore, R>
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
    ) -> Result<Result<SwitchResult, SwitchGateOutcome>, SwitchingError> {
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

        let _lease = match self
            .kernel
            .acquire_session_lease(&request.session_id, SessionMutatingOperation::SwitchModel)
            .await
        {
            Ok(guard) => guard,
            Err(trogonai_session_kernel::SessionKernelError::SessionBusy { session_id, .. }) => {
                return Err(SwitchingError::SessionBusy { session_id });
            }
            Err(err) => return Err(err.into()),
        };
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
            resolve_model_capabilities(&self.registry, &request.target_model, now, &self.capability_config)
                .await?;
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
                .map_err(|err| SwitchingError::Kernel(
                    trogonai_session_kernel::SessionKernelError::ContractValidation(
                        trogonai_session_contracts::ContractValidationError::InvalidIdempotencyKey(err),
                    ),
                ))?,
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

        if request.force {
            self.kernel
                .append_event(force_switch_requested_event(
                    &flow,
                    &request.force_acknowledged_losses,
                ))
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
                    self.kernel
                        .append_event(force_switch_confirmed_event(&flow))
                        .await?;
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
        .await?;
        self.kernel.append_event(attach_event).await?;
        events_appended += 1;
        switch_state = switch_state.transition_to(SwitchState::RunnerAttached)?;
        session_state.active_runner_binding = MessageField::some(binding);

        let continuity_warnings = safety
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

        let projection = self.compiler.compile(ProjectionInput {
            session_id: request.session_id.as_str().to_string(),
            model_id: request.target_model.clone(),
            snapshot: session_state.clone(),
            context_twin: context_twin.clone(),
            adaptation_plan: Some(adaptation_plan.clone()),
            capabilities: target_capabilities.clone(),
            token_budget: target_capabilities.schema.max_context_tokens,
            current_request,
            continuity_warnings,
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
            let started = continuity_checkpoint_started_event(
                &flow,
                &checkpoint_id,
                &request.target_model,
                &target_runner,
            );
            self.kernel.append_event(started).await?;
            events_appended += 1;

            let (result, _) = match run_continuity_checkpoint(
                self.runner.as_ref(),
                request.session_id.as_str(),
                &request.target_model,
                &context_twin,
                &projection,
                &self.switching_config,
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
                Err(err) => return Err(err),
            };
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

        Ok(Ok(SwitchResult {
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
        config.permission_rules_text =
            Some("Apply portable session permission rules.".to_string());
    }
}
