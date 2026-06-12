use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKeyValueUpdate};
use trogonai_session_contracts::{
    NonPortableState, RunnerBinding, RunnerBindingStatus, SCHEMA_VERSION_V1, SessionEvent,
    SessionId, SessionSnapshotState,
};

use crate::config::SwitchingConfig;
use crate::error::SwitchingError;
use crate::event::{runner_attached_event, runner_detached_event};
use crate::nats::{runner_binding_key, runner_bindings_bucket};
use crate::state::RunnerBindingState;
use crate::telemetry;

/// Event context shared by runner attach/detach operations.
#[derive(Clone, Debug)]
pub struct RunnerBindingContext {
    pub session_id: SessionId,
    pub operation_id: trogonai_session_contracts::OperationId,
    pub correlation_id: String,
    pub idempotency_key: trogonai_session_contracts::IdempotencyKey,
    pub causation_id: Option<trogonai_session_contracts::EventId>,
    pub actor: trogonai_session_contracts::Actor,
    pub created_at: Timestamp,
}

/// NATS KV store for runtime runner bindings (`ACP_RUNNER_BINDINGS`).
#[derive(Clone)]
pub struct RunnerBindingStore<S> {
    store: S,
    config: SwitchingConfig,
}

impl<S> RunnerBindingStore<S> {
    pub fn new(store: S, config: SwitchingConfig) -> Self {
        Self { store, config }
    }

    pub fn bucket_name(&self) -> String {
        runner_bindings_bucket(&self.config.nats_prefix)
    }
}

impl<S> RunnerBindingStore<S>
where
    S: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    pub async fn load_binding(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<RunnerBinding>, SwitchingError> {
        let key = runner_binding_key(session_id);
        let Some(bytes) = self
            .store
            .get(key)
            .await
            .map_err(|err| SwitchingError::RunnerBindingLoad(err.to_string()))?
        else {
            return Ok(None);
        };
        let binding = RunnerBinding::decode_from_slice(&bytes)
            .map_err(|err| SwitchingError::Decode(err.to_string()))?;
        Ok(Some(binding))
    }

    pub async fn save_binding(&self, binding: &RunnerBinding) -> Result<(), SwitchingError> {
        if binding.session_id.is_empty() {
            return Err(SwitchingError::MissingField("session_id"));
        }
        let session_id = SessionId::new(&binding.session_id)
            .map_err(|err| SwitchingError::Kernel(
                trogonai_session_kernel::SessionKernelError::ContractValidation(
                    trogonai_session_contracts::ContractValidationError::InvalidSessionId(err),
                ),
            ))?;
        let key = runner_binding_key(&session_id);
        let bytes = Bytes::from(binding.encode_to_vec());
        if let Some(entry) = self
            .store
            .entry(key.clone())
            .await
            .map_err(|err| SwitchingError::RunnerBindingStore(err.to_string()))?
        {
            self.store
                .update(&key, bytes, entry.revision)
                .await
                .map_err(|err| SwitchingError::RunnerBindingStore(err.to_string()))?;
            return Ok(());
        }
        self.store
            .create(&key, bytes)
            .await
            .map_err(|err| SwitchingError::RunnerBindingStore(err.to_string()))?;
        Ok(())
    }
}

/// Attach a disposable runtime runner binding for the target model.
pub async fn attach_runner<S>(
    binding_store: &RunnerBindingStore<S>,
    context: &RunnerBindingContext,
    runner_id: &str,
    model_id: &str,
    capability_snapshot_id: Option<String>,
) -> Result<(RunnerBinding, SessionEvent, RunnerBindingState), SwitchingError>
where
    S: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    let mut state = RunnerBindingState::Attaching;
    let _ = state.can_transition_to(RunnerBindingState::Attached);
    let binding = RunnerBinding {
        schema_version: SCHEMA_VERSION_V1,
        session_id: context.session_id.as_str().to_string(),
        runner_id: runner_id.to_string(),
        model_id: model_id.to_string(),
        status: EnumValue::Known(RunnerBindingStatus::Attached),
        attached_at: MessageField::some(context.created_at.clone()),
        capability_snapshot_id,
        ..RunnerBinding::default()
    };

    binding_store.save_binding(&binding).await.map_err(|err| {
        SwitchingError::RunnerAttachFailed {
            runner_id: runner_id.to_string(),
            detail: err.to_string(),
        }
    })?;

    let event = runner_attached_event(context, runner_id, model_id, binding.capability_snapshot_id.clone());
    state = RunnerBindingState::Attached;
    telemetry::metrics::record_runner_attached(context.session_id.as_str(), runner_id, model_id);
    Ok((binding, event, state))
}

/// Detach the active runner binding and invalidate non-portable runner state.
pub async fn detach_runner<S>(
    binding_store: &RunnerBindingStore<S>,
    context: &RunnerBindingContext,
    runner_id: &str,
    model_id: &str,
    reason: &str,
    session_state: &mut SessionSnapshotState,
) -> Result<(SessionEvent, RunnerBindingState), SwitchingError>
where
    S: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    let mut state = RunnerBindingState::Detaching;
    let _ = state.can_transition_to(RunnerBindingState::Detached);
    invalidate_nonportable_runner_state(session_state);

    if let Some(binding) = binding_store.load_binding(&context.session_id).await? {
        let detached = RunnerBinding {
            status: EnumValue::Known(RunnerBindingStatus::Detached),
            detached_at: MessageField::some(context.created_at.clone()),
            detach_reason: Some(reason.to_string()),
            ..binding
        };
        binding_store.save_binding(&detached).await?;
    }

    let event = runner_detached_event(context, runner_id, model_id, reason);
    state = RunnerBindingState::Detached;
    telemetry::metrics::record_runner_detached(context.session_id.as_str(), runner_id, model_id);
    Ok((event, state))
}

/// Clear provider-specific runtime state that must not survive a runner change.
pub fn invalidate_nonportable_runner_state(session_state: &mut SessionSnapshotState) {
    session_state.nonportable = MessageField::some(NonPortableState::default());
}

pub async fn provision_runner_binding_store<J, S>(
    js: &J,
    config: &SwitchingConfig,
) -> Result<S, SwitchingError>
where
    J: trogon_nats::jetstream::JetStreamGetKeyValue<Store = S>
        + trogon_nats::jetstream::JetStreamCreateKeyValue<Store = S>,
    S: trogon_nats::jetstream::JetStreamKeyValueStatus,
{
    let bucket = runner_bindings_bucket(&config.nats_prefix);
    match js.get_key_value(bucket.clone()).await {
        Ok(store) => Ok(store),
        Err(_) => js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .map_err(|err| SwitchingError::RunnerBindingStore(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidate_nonportable_clears_runtime_ids() {
        let mut state = SessionSnapshotState {
            nonportable: MessageField::some(NonPortableState {
                provider_response_ids: vec!["resp_1".to_string()],
                terminal_ids: vec!["term_1".to_string()],
                live_processes: vec!["bash".to_string()],
                ..NonPortableState::default()
            }),
            ..SessionSnapshotState::default()
        };
        invalidate_nonportable_runner_state(&mut state);
        let cleared = state.nonportable.as_option().unwrap();
        assert!(cleared.provider_response_ids.is_empty());
        assert!(cleared.terminal_ids.is_empty());
        assert!(cleared.live_processes.is_empty());
    }
}
