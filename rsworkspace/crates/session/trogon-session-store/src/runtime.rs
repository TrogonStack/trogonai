use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use buffa::{Message, MessageName};
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, CommandId, Event, Headers, StreamAppend, StreamPosition, StreamRead,
    StreamWritePrecondition,
};
use trogon_identity_types::SubjectPrincipal;

use crate::{
    ChildAdmission, CreateAdmission, DispatchAdmission, SessionAdmission, SessionError, SessionFactCodec, SessionKey,
    history::{self, History},
    lease::guarded,
    original::{OriginalFields, Wire, bytes_field},
    wire,
};

pub struct SessionRuntime<S, A, C> {
    store: Arc<S>,
    admission: Arc<A>,
    codec: Arc<C>,
}

#[derive(Debug)]
pub struct RecoveredChild {
    pub child_key: SessionKey,
    pub creation_position: StreamPosition,
    pub inserted: bool,
}

#[derive(Debug)]
pub struct RecordedDispatch {
    pub ordinal: u64,
    pub position: StreamPosition,
    pub inserted: bool,
}

impl<S, A, C> SessionRuntime<S, A, C> {
    pub fn new(store: Arc<S>, admission: Arc<A>, codec: Arc<C>) -> Self {
        Self {
            store,
            admission,
            codec,
        }
    }
}

impl<S, A, C> SessionRuntime<S, A, C>
where
    S: StreamRead<SessionKey> + StreamAppend<SessionKey>,
    A: SessionAdmission,
    C: SessionFactCodec,
{
    /// Persists creation under current admission without starting an execution attempt.
    /// A post-append error may leave durable facts; reconcile with the same command identity.
    pub async fn create_session(
        &self,
        caller: &SubjectPrincipal,
        key: &SessionKey,
        command_id: CommandId,
        original_command: &[u8],
    ) -> Result<StreamPosition, SessionError> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let wire = Wire::new()?;
        let command: wire::CreateSession = wire.decode(original_command)?;
        let started: wire::SessionStarted = wire.decode(original_command)?;
        history::validate_started(key, &started)?;
        let admitted = guarded::<A::Lease, _>(&[], deadline, async {
            self.admission
                .authorize_create(CreateAdmission {
                    caller,
                    key,
                    command: &command,
                    original_command,
                })
                .await
                .map_err(|error| SessionError::Admission(Box::new(error)))
        })
        .await?;
        let source = guarded(&[&admitted], deadline, async {
            self.admission
                .authorize_source(caller, key)
                .await
                .map_err(|error| SessionError::Admission(Box::new(error)))
        })
        .await?;
        let leases = [&admitted, &source];
        let originals = [(wire::SessionStarted::FULL_NAME, original_command.to_vec())];
        if let Some(existing) =
            history::load(self.store.as_ref(), self.codec.as_ref(), &wire, key, &leases, deadline).await?
        {
            return prefix_matches(&existing, command_id, &originals);
        }
        let events = self.seal(key, command_id, &originals, &leases, deadline).await?;
        let result = guarded(&leases, deadline, async {
            Ok(self
                .store
                .append_stream(AppendStreamRequest {
                    stream_id: key,
                    stream_write_precondition: StreamWritePrecondition::NoStream,
                    events: events.clone(),
                })
                .await)
        })
        .await?;
        match result {
            Ok(result) => Ok(result.stream_position),
            Err(error) => {
                let unknown = self.store.classify_append_failure(&error) != AppendFailure::WriteConflict;
                let existing =
                    history::load(self.store.as_ref(), self.codec.as_ref(), &wire, key, &leases, deadline).await?;
                match existing {
                    Some(existing) => prefix_matches(&existing, command_id, &originals),
                    None if unknown => Err(SessionError::AppendUnknown),
                    None => Err(SessionError::SourceChanged),
                }
            }
        }
    }

    /// Records an admitted coordinator dispatch under the parent's current stream guard.
    /// This records intent and grants no Ready or child execution authority.
    pub async fn dispatch_delegation(
        &self,
        caller: &SubjectPrincipal,
        key: &SessionKey,
        command_id: CommandId,
        original_command: &[u8],
    ) -> Result<RecordedDispatch, SessionError> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let wire = Wire::new()?;
        let command: wire::DispatchDelegation = wire.decode(original_command)?;
        let fields = OriginalFields::parse(original_command)?;
        let mut original = Vec::new();
        for (source, destination) in [(1, 1), (2, 2), (3, 3), (4, 5), (5, 6), (6, 7), (7, 8)] {
            fields.copy(source, destination, &mut original)?;
        }
        let dispatch: wire::DelegationDispatched = wire.decode(&original)?;
        let source = guarded::<A::Lease, _>(&[], deadline, async {
            self.admission
                .authorize_source(caller, key)
                .await
                .map_err(|error| SessionError::Admission(Box::new(error)))
        })
        .await?;
        let parent = history::load(
            self.store.as_ref(),
            self.codec.as_ref(),
            &wire,
            key,
            &[&source],
            deadline,
        )
        .await?
        .ok_or(SessionError::UnsupportedHistory)?;
        history::validate_dispatch(key, &parent.started.value, &dispatch)?;
        if parent.terminal {
            return Err(SessionError::SourceChanged);
        }
        let admitted = guarded(&[&source], deadline, async {
            self.admission
                .authorize_dispatch(DispatchAdmission {
                    caller,
                    key,
                    parent: &parent.started.value,
                    original_parent: &parent.started.original,
                    command: &command,
                    original_command,
                    attempt: parent.attempt_source(),
                })
                .await
                .map_err(|error| SessionError::Admission(Box::new(error)))
        })
        .await?;
        let leases = [&source, &admitted];
        if let Some(existing) = parent.dispatches.get(&command.operation_id) {
            if existing.original.as_slice() != original || existing.event.id != command_id.event_id(0) {
                return Err(SessionError::CreationConflict);
            }
            let current = history::load(self.store.as_ref(), self.codec.as_ref(), &wire, key, &leases, deadline)
                .await?
                .ok_or(SessionError::SourceChanged)?;
            if current.position != parent.position || current.terminal {
                return Err(SessionError::SourceChanged);
            }
            return Ok(RecordedDispatch {
                ordinal: existing.ordinal,
                position: parent.originals[existing.ordinal as usize - 1].2,
                inserted: false,
            });
        }
        if parent
            .dispatches
            .values()
            .any(|fact| fact.value.child_session_id == command.child_session_id)
        {
            return Err(SessionError::CreationConflict);
        }
        let originals = [(wire::DelegationDispatched::FULL_NAME, original.clone())];
        let events = self.seal(key, command_id, &originals, &leases, deadline).await?;
        let result = guarded(&leases, deadline, async {
            Ok(self
                .store
                .append_stream(AppendStreamRequest {
                    stream_id: key,
                    stream_write_precondition: StreamWritePrecondition::At(parent.position),
                    events: events.clone(),
                })
                .await)
        })
        .await?;
        match result {
            Ok(result) => Ok(RecordedDispatch {
                ordinal: parent.originals.len() as u64 + 1,
                position: result.stream_position,
                inserted: true,
            }),
            Err(error) => {
                let unknown = self.store.classify_append_failure(&error) != AppendFailure::WriteConflict;
                let current = history::load(self.store.as_ref(), self.codec.as_ref(), &wire, key, &leases, deadline)
                    .await?
                    .ok_or(SessionError::AppendUnknown)?;
                if let Some(existing) = current.dispatches.get(&command.operation_id) {
                    if existing.original.as_slice() != original || existing.event.id != command_id.event_id(0) {
                        return Err(SessionError::CreationConflict);
                    }
                    Ok(RecordedDispatch {
                        ordinal: existing.ordinal,
                        position: current.originals[existing.ordinal as usize - 1].2,
                        inserted: false,
                    })
                } else if unknown {
                    Err(SessionError::AppendUnknown)
                } else {
                    Err(SessionError::SourceChanged)
                }
            }
        }
    }

    /// Replays original parent intent into an atomic child creation, without launching it.
    /// Cross-stream movement or authority loss can be reported after the child became durable.
    pub async fn recover_child(
        &self,
        caller: &SubjectPrincipal,
        parent_key: &SessionKey,
        operation_id: &str,
    ) -> Result<RecoveredChild, SessionError> {
        let deadline = Instant::now() + Duration::from_secs(15);
        if !history::valid_id(operation_id) {
            return Err(SessionError::MalformedWire);
        }
        let wire = Wire::new()?;
        let source = guarded::<A::Lease, _>(&[], deadline, async {
            self.admission
                .authorize_source(caller, parent_key)
                .await
                .map_err(|error| SessionError::Admission(Box::new(error)))
        })
        .await?;
        let parent = history::load(
            self.store.as_ref(),
            self.codec.as_ref(),
            &wire,
            parent_key,
            &[&source],
            deadline,
        )
        .await?
        .ok_or(SessionError::UnsupportedHistory)?;
        if parent.terminal {
            return Err(SessionError::SourceChanged);
        }
        let dispatch = parent
            .dispatches
            .get(operation_id)
            .ok_or(SessionError::UnsupportedHistory)?;
        let child_key = SessionKey::new(parent_key.namespace().clone(), &dispatch.value.child_session_id)?;
        let (started, link, creation) = child_originals(&wire, dispatch)?;
        let admitted = guarded(&[&source], deadline, async {
            self.admission
                .authorize_child(ChildAdmission {
                    caller,
                    parent_key,
                    child_key: &child_key,
                    parent: &parent.started.value,
                    original_parent: &parent.started.original,
                    dispatch: &dispatch.value,
                    original_dispatch: &dispatch.original,
                    creation: &creation,
                    dispatch_event_id: dispatch.event.id,
                    dispatch_ordinal: dispatch.ordinal,
                    parent_position: parent.position,
                    attempt: parent.attempt_source(),
                })
                .await
                .map_err(|error| SessionError::Admission(Box::new(error)))
        })
        .await?;
        let child_source = guarded(&[&source, &admitted], deadline, async {
            self.admission
                .authorize_source(caller, &child_key)
                .await
                .map_err(|error| SessionError::Admission(Box::new(error)))
        })
        .await?;
        let leases = [&source, &admitted, &child_source];
        let command_id = CommandId::new(dispatch.event.id.as_uuid());
        let originals = [
            (wire::SessionStarted::FULL_NAME, started),
            (wire::ParentLinked::FULL_NAME, link),
        ];
        if let Some(existing) = history::load(
            self.store.as_ref(),
            self.codec.as_ref(),
            &wire,
            &child_key,
            &leases,
            deadline,
        )
        .await?
        {
            let creation_position = prefix_matches(&existing, command_id, &originals)?;
            self.check_parent(&wire, parent_key, &parent, &leases, deadline).await?;
            return Ok(RecoveredChild {
                child_key,
                creation_position,
                inserted: false,
            });
        }
        let events = self.seal(&child_key, command_id, &originals, &leases, deadline).await?;
        self.check_parent(&wire, parent_key, &parent, &leases, deadline).await?;
        let result = guarded(&leases, deadline, async {
            Ok(self
                .store
                .append_stream(AppendStreamRequest {
                    stream_id: &child_key,
                    stream_write_precondition: StreamWritePrecondition::NoStream,
                    events: events.clone(),
                })
                .await)
        })
        .await?;
        let (creation_position, inserted) = match result {
            Ok(result) => (result.stream_position, true),
            Err(error) => {
                let unknown = self.store.classify_append_failure(&error) != AppendFailure::WriteConflict;
                let current = history::load(
                    self.store.as_ref(),
                    self.codec.as_ref(),
                    &wire,
                    &child_key,
                    &leases,
                    deadline,
                )
                .await?;
                match current {
                    Some(current) => (prefix_matches(&current, command_id, &originals)?, false),
                    None if unknown => return Err(SessionError::AppendUnknown),
                    None => return Err(SessionError::SourceChanged),
                }
            }
        };
        // A changed parent can leave a durable child creation, which grants no execution authority.
        self.check_parent(&wire, parent_key, &parent, &leases, deadline).await?;
        Ok(RecoveredChild {
            child_key,
            creation_position,
            inserted,
        })
    }

    async fn check_parent(
        &self,
        wire: &Wire,
        key: &SessionKey,
        original: &History,
        leases: &[&A::Lease],
        deadline: Instant,
    ) -> Result<(), SessionError> {
        let current = history::load(self.store.as_ref(), self.codec.as_ref(), wire, key, leases, deadline)
            .await?
            .ok_or(SessionError::SourceChanged)?;
        if current.position != original.position
            || current.terminal
            || current.started.original != original.started.original
            || current.started.event != original.started.event
        {
            return Err(SessionError::SourceChanged);
        }
        Ok(())
    }

    async fn seal(
        &self,
        key: &SessionKey,
        command_id: CommandId,
        originals: &[(&str, Vec<u8>)],
        leases: &[&A::Lease],
        deadline: Instant,
    ) -> Result<Vec<Event>, SessionError> {
        let mut events = Vec::new();
        for (index, (event_type, original)) in originals.iter().enumerate() {
            if original.len() > 131072 {
                return Err(SessionError::Limit);
            }
            let id = command_id.event_id(index);
            let content = guarded(leases, deadline, async {
                self.codec
                    .seal(key, id, event_type, original)
                    .await
                    .map_err(|error| SessionError::Authentication(Box::new(error)))
            })
            .await?;
            if content.len() > 262144 {
                return Err(SessionError::Limit);
            }
            events.push(Event {
                id,
                r#type: (*event_type).to_owned(),
                content,
                headers: Headers::empty(),
            });
        }
        Ok(events)
    }
}

fn prefix_matches(
    history: &History,
    command_id: CommandId,
    originals: &[(&str, Vec<u8>)],
) -> Result<StreamPosition, SessionError> {
    if history.originals.len() < originals.len() {
        return Err(SessionError::CreationConflict);
    }
    for (index, (kind, original)) in originals.iter().enumerate() {
        let (recorded, plaintext, _) = &history.originals[index];
        if recorded.id != command_id.event_id(index) || recorded.r#type != *kind || plaintext.as_slice() != original {
            return Err(SessionError::CreationConflict);
        }
    }
    history
        .originals
        .get(originals.len() - 1)
        .map(|fact| fact.2)
        .ok_or(SessionError::CreationConflict)
}

fn child_originals(
    wire: &Wire,
    dispatch: &history::OriginalFact<wire::DelegationDispatched>,
) -> Result<(Vec<u8>, Vec<u8>, wire::CreateChildSession), SessionError> {
    let fields = OriginalFields::parse(&dispatch.original)?;
    let creation_fields = OriginalFields::parse(fields.message(8)?)?;
    let mut started = Vec::new();
    fields.copy(3, 1, &mut started)?;
    creation_fields.copy(1, 2, &mut started)?;
    creation_fields.copy(2, 3, &mut started)?;
    creation_fields.copy_optional(3, 4, &mut started)?;
    let mut link = Vec::new();
    fields.copy(3, 1, &mut link)?;
    fields.copy(1, 2, &mut link)?;
    bytes_field(
        3,
        &wire::SessionOrdinal {
            value: dispatch.ordinal,
        }
        .encode_to_vec(),
        &mut link,
    );
    fields.copy(5, 4, &mut link)?;
    fields.copy(2, 5, &mut link)?;
    fields.copy(7, 6, &mut link)?;
    let started_value: wire::SessionStarted = wire.decode(&started)?;
    let link_value: wire::ParentLinked = wire.decode(&link)?;
    let creation = wire::CreateChildSession {
        session_id: started_value.session_id,
        execution_plan: started_value.execution_plan,
        checkout_ref: started_value.checkout_ref,
        parent_session_id: link_value.parent_session_id,
        parent_dispatched_at: link_value.parent_dispatched_at,
        cascade_policy: link_value.cascade_policy,
        operation_id: link_value.operation_id,
        candidate_ref: started_value.candidate_ref,
        delegate_binding: link_value.delegate_binding,
    };
    Ok((started, link, creation))
}

// Strict public ingress already bounds originals before the final sealing boundary.
#[cfg(test)]
mod tests;
