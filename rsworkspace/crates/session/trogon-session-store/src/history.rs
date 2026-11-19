use std::collections::{BTreeMap, BTreeSet};

use buffa::MessageName;
use sha2::{Digest as _, Sha256};
use trogon_decider_runtime::{Event, ReadFrom, ReadStreamRequest, StreamPosition, StreamRead};
use zeroize::Zeroizing;

use crate::{SessionAdmissionLease, SessionError, SessionFactCodec, SessionKey, lease::guarded, original::Wire, wire};

pub(crate) struct OriginalFact<T> {
    pub value: T,
    pub original: Zeroizing<Vec<u8>>,
    pub event: Event,
    pub ordinal: u64,
}

pub(crate) struct History {
    pub started: OriginalFact<wire::SessionStarted>,
    pub dispatches: BTreeMap<String, OriginalFact<wire::DelegationDispatched>>,
    pub position: StreamPosition,
    pub terminal: bool,
    pub originals: Vec<(Event, Zeroizing<Vec<u8>>, StreamPosition)>,
    pub attempt: Option<OriginalFact<wire::ExecutionAttemptStarted>>,
    pub ready: Option<OriginalFact<wire::ExecutionAttemptReady>>,
}

impl History {
    pub fn attempt_source(&self) -> Option<crate::AttemptSource<'_>> {
        self.attempt.as_ref().map(|attempt| crate::AttemptSource {
            started: &attempt.value,
            original_started: &attempt.original,
            ready: self.ready.as_ref().map(|ready| &ready.value),
            original_ready: self.ready.as_ref().map(|ready| ready.original.as_slice()),
        })
    }
}

pub(crate) async fn load<S, C, L>(
    store: &S,
    codec: &C,
    wire: &Wire,
    key: &SessionKey,
    leases: &[&L],
    deadline: std::time::Instant,
) -> Result<Option<History>, SessionError>
where
    S: StreamRead<SessionKey>,
    C: SessionFactCodec,
    L: SessionAdmissionLease,
{
    let response = guarded(leases, deadline, async {
        store
            .read_stream_bounded(
                ReadStreamRequest {
                    stream_id: key,
                    from: ReadFrom::Beginning,
                },
                1025,
            )
            .await
            .map_err(|error| SessionError::Persistence(Box::new(error)))
    })
    .await?;
    if response.events.len() > 1024 {
        return Err(SessionError::Limit);
    }
    if response.events.is_empty() {
        return if response.current_position.is_none() {
            Ok(None)
        } else {
            Err(SessionError::UnsupportedHistory)
        };
    }
    let mut started = None;
    let mut dispatches = BTreeMap::new();
    let mut terminal = false;
    let mut previous = None;
    let mut ids = BTreeSet::new();
    let mut children = BTreeSet::new();
    let mut originals = Vec::new();
    let mut total = 0usize;
    let mut linked = false;
    let mut attempt: Option<OriginalFact<wire::ExecutionAttemptStarted>> = None;
    let mut ready: Option<OriginalFact<wire::ExecutionAttemptReady>> = None;
    let mut attempts = BTreeSet::new();
    let mut last_attempt_id = None;
    let mut last_attempt_number = 0u64;
    let mut reservations = BTreeSet::new();
    for (index, source) in response.events.into_iter().enumerate() {
        total = total.saturating_add(source.event.content.len());
        if total > 64 * 1024 * 1024 || source.event.content.len() > 262144 {
            return Err(SessionError::Limit);
        }
        if source.stream_id != key.to_string()
            || previous.is_some_and(|position| position >= source.stream_position)
            || !ids.insert(source.event.id)
            || terminal
        {
            return Err(SessionError::UnsupportedHistory);
        }
        let original = guarded(leases, deadline, async {
            codec
                .open(key, source.event.id, &source.event.r#type, &source.event.content)
                .await
                .map(Zeroizing::new)
                .map_err(|error| SessionError::Authentication(Box::new(error)))
        })
        .await?;
        let ordinal = index as u64 + 1;
        match source.event.r#type.as_str() {
            wire::SessionStarted::FULL_NAME if index == 0 => {
                let value: wire::SessionStarted = wire.decode(&original)?;
                validate_started(key, &value)?;
                started = Some(OriginalFact {
                    value,
                    original: original.clone(),
                    event: source.event.clone(),
                    ordinal,
                });
            }
            wire::ParentLinked::FULL_NAME if index == 1 && !linked => {
                let value: wire::ParentLinked = wire.decode(&original)?;
                if value.session_id != key.session_id()
                    || value.parent_session_id == key.session_id()
                    || !valid_id(&value.parent_session_id)
                    || !valid_id(&value.operation_id)
                    || value.parent_dispatched_at.value == 0
                {
                    return Err(SessionError::UnsupportedHistory);
                }
                validate_binding(&value.delegate_binding)?;
                validate_cascade(value.cascade_policy.as_known())?;
                linked = true;
            }
            wire::DelegationDispatched::FULL_NAME if started.is_some() => {
                let value: wire::DelegationDispatched = wire.decode(&original)?;
                validate_dispatch(
                    key,
                    &started.as_ref().ok_or(SessionError::UnsupportedHistory)?.value,
                    &value,
                )?;
                let operation = value.operation_id.clone();
                if !children.insert(value.child_session_id.clone())
                    || dispatches
                        .insert(
                            operation,
                            OriginalFact {
                                value,
                                original: original.clone(),
                                event: source.event.clone(),
                                ordinal,
                            },
                        )
                        .is_some()
                {
                    return Err(SessionError::UnsupportedHistory);
                }
            }
            wire::ExecutionAttemptStarted::FULL_NAME if started.is_some() => {
                let value: wire::ExecutionAttemptStarted = wire.decode(&original)?;
                if attempt.is_some()
                    || value.session_id != key.session_id()
                    || !valid_id(&value.execution_attempt_id)
                    || value.session_execution_plan_digest
                        != started
                            .as_ref()
                            .ok_or(SessionError::UnsupportedHistory)?
                            .value
                            .execution_plan
                            .plan_digest
                    || value.attempt_number != last_attempt_number.checked_add(1).ok_or(SessionError::Limit)?
                    || value.previous_attempt_id.as_deref().unwrap_or("") != last_attempt_id.as_deref().unwrap_or("")
                    || value.restored_checkpoint.is_set()
                    || !value.started_at.is_set()
                    || !value.host_artifact_ref.is_set()
                    || !attempts.insert(value.execution_attempt_id.clone())
                {
                    return Err(SessionError::UnsupportedHistory);
                }
                validate_claim(&value.host_artifact_ref)?;
                if !(0..1_000_000_000).contains(&value.started_at.nanos) {
                    return Err(SessionError::UnsupportedHistory);
                }
                last_attempt_id = Some(value.execution_attempt_id.clone());
                last_attempt_number = value.attempt_number;
                attempt = Some(OriginalFact {
                    value,
                    original: original.clone(),
                    event: source.event.clone(),
                    ordinal,
                });
            }
            wire::ExecutionAttemptReady::FULL_NAME if attempt.is_some() => {
                let value: wire::ExecutionAttemptReady = wire.decode(&original)?;
                if ready.is_some()
                    || value.session_id != key.session_id()
                    || value.execution_attempt_id
                        != attempt
                            .as_ref()
                            .ok_or(SessionError::UnsupportedHistory)?
                            .value
                            .execution_attempt_id
                    || !value.ready_at.is_set()
                    || !value.ready_attestation_ref.is_set()
                {
                    return Err(SessionError::UnsupportedHistory);
                }
                validate_claim(&value.ready_attestation_ref)?;
                if !(0..1_000_000_000).contains(&value.ready_at.nanos) {
                    return Err(SessionError::UnsupportedHistory);
                }
                ready = Some(OriginalFact {
                    value,
                    original: original.clone(),
                    event: source.event.clone(),
                    ordinal,
                });
            }
            wire::ExecutionAttemptEnded::FULL_NAME if attempt.is_some() => {
                let value: wire::ExecutionAttemptEnded = wire.decode(&original)?;
                if value.session_id != key.session_id()
                    || value.execution_attempt_id
                        != attempt
                            .as_ref()
                            .ok_or(SessionError::UnsupportedHistory)?
                            .value
                            .execution_attempt_id
                    || value
                        .outcome
                        .as_known()
                        .is_none_or(|outcome| outcome == wire::AttemptOutcome::ATTEMPT_OUTCOME_UNSPECIFIED)
                    || !value.ended_at.is_set()
                {
                    return Err(SessionError::UnsupportedHistory);
                }
                if !(0..1_000_000_000).contains(&value.ended_at.nanos) {
                    return Err(SessionError::UnsupportedHistory);
                }
                attempt = None;
                ready = None;
            }
            wire::OperationReserved::FULL_NAME if started.is_some() => {
                let value: wire::OperationReserved = wire.decode(&original)?;
                if value.session_id != key.session_id()
                    || !valid_id(&value.operation_id)
                    || !valid_id(&value.turn_id)
                    || !valid_digest(&value.request_digest)
                    || value.operation_kind.as_known()
                        != Some(wire::OperationKind::OPERATION_KIND_CHILD_SESSION_DELEGATION)
                    || !reservations.insert(value.operation_id)
                {
                    return Err(SessionError::UnsupportedHistory);
                }
            }
            wire::SessionClosed::FULL_NAME if started.is_some() => {
                let value: wire::SessionClosed = wire.decode(&original)?;
                if value.session_id != key.session_id() {
                    return Err(SessionError::UnsupportedHistory);
                }
                terminal = true;
            }
            wire::SessionCancelled::FULL_NAME if started.is_some() => {
                let value: wire::SessionCancelled = wire.decode(&original)?;
                if value.session_id != key.session_id()
                    || value.reason.as_known().is_none_or(|reason| {
                        reason == wire::SessionCancellationReason::SESSION_CANCELLATION_REASON_UNSPECIFIED
                    })
                {
                    return Err(SessionError::UnsupportedHistory);
                }
                terminal = true;
            }
            wire::SessionFailed::FULL_NAME if started.is_some() => {
                let value: wire::SessionFailed = wire.decode(&original)?;
                if value.session_id != key.session_id()
                    || value
                        .reason
                        .as_known()
                        .is_none_or(|reason| reason == wire::SessionFailureReason::SESSION_FAILURE_REASON_UNSPECIFIED)
                {
                    return Err(SessionError::UnsupportedHistory);
                }
                terminal = true;
            }
            _ => return Err(SessionError::UnsupportedHistory),
        }
        originals.push((source.event, original, source.stream_position));
        previous = Some(source.stream_position);
    }
    if previous != response.current_position {
        return Err(SessionError::UnsupportedHistory);
    }
    Ok(Some(History {
        started: started.ok_or(SessionError::UnsupportedHistory)?,
        dispatches,
        position: previous.ok_or(SessionError::UnsupportedHistory)?,
        terminal,
        originals,
        attempt,
        ready,
    }))
}

pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub(crate) fn validate_started(key: &SessionKey, value: &wire::SessionStarted) -> Result<(), SessionError> {
    if value.session_id != key.session_id()
        || !value.execution_plan.is_set()
        || !value.checkout_ref.is_set()
        || !valid_id(&value.checkout_ref.checkout_id)
        || value.checkout_ref.uri.len() > 4096
        || value
            .checkout_ref
            .revision
            .as_ref()
            .is_some_and(|revision| revision.len() > 512)
    {
        return Err(SessionError::UnsupportedHistory);
    }
    validate_plan(&value.execution_plan)?;
    if let Some(candidate) = value.candidate_ref.as_option()
        && (!valid_id(&candidate.candidate_id.value) || !valid_digest(&candidate.content_digest))
    {
        return Err(SessionError::UnsupportedHistory);
    }
    Ok(())
}

pub(crate) fn validate_plan(value: &wire::StoredSessionExecutionPlan) -> Result<(), SessionError> {
    if value.plan_bytes.is_empty()
        || value.plan_bytes.len() > 65536
        || !valid_digest(&value.plan_digest)
        || Sha256::digest(&value.plan_bytes).as_slice() != value.plan_digest.value
    {
        return Err(SessionError::UnsupportedHistory);
    }
    Ok(())
}

fn valid_digest(value: &wire::Digest) -> bool {
    value.algorithm == "sha256" && value.value.len() == 32
}

fn validate_claim(value: &wire::ClaimCheck) -> Result<(), SessionError> {
    if !value.storage_ref.is_set()
        || value.storage_ref.value.is_empty()
        || value.storage_ref.value.len() > 4096
        || !value.digest.is_set()
        || !valid_digest(&value.digest)
    {
        return Err(SessionError::UnsupportedHistory);
    }
    Ok(())
}

fn validate_binding(value: &wire::DelegationBindingRef) -> Result<(), SessionError> {
    if !value
        .binding_name
        .as_deref()
        .is_some_and(|name| !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control))
        || !value.parent_execution_plan_digest.is_set()
        || !valid_digest(&value.parent_execution_plan_digest)
    {
        return Err(SessionError::UnsupportedHistory);
    }
    Ok(())
}

fn validate_cascade(value: Option<wire::CascadePolicy>) -> Result<(), SessionError> {
    match value {
        Some(
            wire::CascadePolicy::CASCADE_POLICY_CASCADE_ON_PARENT_TERMINAL
            | wire::CascadePolicy::CASCADE_POLICY_INDEPENDENT,
        ) => Ok(()),
        _ => Err(SessionError::UnsupportedHistory),
    }
}

pub(crate) fn validate_dispatch(
    key: &SessionKey,
    parent: &wire::SessionStarted,
    value: &wire::DelegationDispatched,
) -> Result<(), SessionError> {
    if value.session_id != key.session_id()
        || value.child_session_id == key.session_id()
        || !valid_id(&value.child_session_id)
        || !valid_id(&value.operation_id)
        || !valid_id(&value.turn_id)
        || !value.delegate_binding.is_set()
        || !value.child_creation.is_set()
        || value.delegate_binding.parent_execution_plan_digest != parent.execution_plan.plan_digest
    {
        return Err(SessionError::UnsupportedHistory);
    }
    validate_binding(&value.delegate_binding)?;
    validate_cascade(value.cascade_policy.as_known())?;
    let creation = value
        .child_creation
        .as_option()
        .ok_or(SessionError::UnsupportedHistory)?;
    let child = wire::SessionStarted {
        session_id: value.child_session_id.clone(),
        execution_plan: creation.execution_plan.clone(),
        checkout_ref: creation.checkout_ref.clone(),
        candidate_ref: creation.candidate_ref.clone(),
    };
    validate_started(
        &SessionKey::new(key.namespace().clone(), &value.child_session_id)?,
        &child,
    )
}
