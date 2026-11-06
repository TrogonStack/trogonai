use std::num::NonZeroU64;

use trogon_audit::{
    Actor, AuditName, AuditWriter, ChangeFacts, ChangeTerminal, JetStreamSink, Record, RecordScope, Stamper,
    WriterConfig, WriterId, WriterIdentity,
};
use trogon_std::{SystemClock, UuidV7Generator};

use crate::{ApprovedRegistration, ProducerConfig, RegistrationError, RegistrationRole, StreamProfile};

pub(crate) fn production_writer(
    context: async_nats::jetstream::Context,
    config: &ProducerConfig,
    role: RegistrationRole,
) -> Result<AuditWriter<JetStreamSink<async_nats::jetstream::Context>>, RegistrationError> {
    let component = StreamProfile::new(config).audit_component(role)?;
    let identity = WriterIdentity::new(
        WriterId::new(component.as_str()).map_err(RegistrationError::AuditName)?,
        component,
        AuditName::new(config.scope().tenant().as_str()).map_err(RegistrationError::AuditName)?,
        config.scope().project().clone(),
    );
    Ok(AuditWriter::new(
        JetStreamSink::with_defaults(context),
        identity,
        WriterConfig::default(),
        &Stamper::new(SystemClock, UuidV7Generator),
    ))
}

pub(crate) fn change(
    config: &ProducerConfig,
    registration: &ApprovedRegistration,
    sequence: NonZeroU64,
    withdrawal: bool,
) -> Result<Record, RegistrationError> {
    let profile = StreamProfile::new(config);
    let scope = RecordScope::new(
        AuditName::new(config.scope().tenant().as_str()).map_err(RegistrationError::AuditName)?,
        config.scope().project().clone(),
        Actor::ApiKey {
            api_key_id: AuditName::new(format!(
                "ed25519_sha256:{}",
                config.approval_key().fingerprint().to_hex()
            ))
            .map_err(RegistrationError::AuditName)?,
        },
    );
    let facts = ChangeFacts {
        kind: ChangeTerminal::Policy,
        resource_name: AuditName::new(profile.live_bucket()).map_err(RegistrationError::AuditName)?,
        event_type: AuditName::new(if withdrawal {
            "workload_registration_withdrawal"
        } else {
            "workload_registration_grant_intent"
        })
        .map_err(RegistrationError::AuditName)?,
        source_stream: AuditName::new(if withdrawal {
            profile.live_config().name
        } else {
            profile.control_config().name
        })
        .map_err(RegistrationError::AuditName)?,
        stream_sequence: sequence.get(),
        event_id: AuditName::new(format!("sha256:{}", registration.digest().to_hex()))
            .map_err(RegistrationError::AuditName)?,
    };
    Ok(Record::change(
        scope,
        facts,
        Stamper::new(SystemClock, UuidV7Generator).stamp(),
    ))
}
