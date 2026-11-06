use std::marker::PhantomData;
use std::time::Instant;

use trogon_audit::{AuditSink, AuditWriter, JetStreamSink};
use trogon_nats::NatsConfig;

use crate::{ApprovedRegistration, ProducerConfig, ProducerStatus, PublicationReceipt, RegistrationError};

#[derive(Debug)]
pub struct RegistrationProducer<S> {
    _sink: PhantomData<S>,
}

pub type ProductionRegistrationProducer = RegistrationProducer<JetStreamSink<async_nats::jetstream::Context>>;

impl ProductionRegistrationProducer {
    pub async fn start(_config: ProducerConfig, _nats: NatsConfig, _initial: ApprovedRegistration) -> Result<Self, RegistrationError> {
        Err(RegistrationError::NotImplemented)
    }
}

impl<S: AuditSink> RegistrationProducer<S> {
    pub async fn claim(
        _config: ProducerConfig,
        _nats: NatsConfig,
        _initial: ApprovedRegistration,
        _audit: AuditWriter<S>,
    ) -> Result<Self, RegistrationError> {
        Err(RegistrationError::NotImplemented)
    }

    pub async fn apply(&mut self, _next: ApprovedRegistration) -> Result<PublicationReceipt, RegistrationError> {
        Err(RegistrationError::NotImplemented)
    }

    pub fn status(&self) -> ProducerStatus {
        ProducerStatus::Poisoned
    }

    pub fn current_deadline(&self) -> Instant { Instant::now() }
}

pub async fn provision_streams(_nats: &NatsConfig, _config: &ProducerConfig) -> Result<(), RegistrationError> {
    Err(RegistrationError::NotImplemented)
}

pub async fn emergency_withdrawal<S: AuditSink>(
    _nats: &NatsConfig,
    _config: &ProducerConfig,
    _denial: ApprovedRegistration,
    _audit: &AuditWriter<S>,
) -> Result<PublicationReceipt, RegistrationError> {
    Err(RegistrationError::NotImplemented)
}

pub async fn production_emergency_withdrawal(_nats: &NatsConfig, _config: &ProducerConfig, _denial: ApprovedRegistration) -> Result<PublicationReceipt, RegistrationError> {
    Err(RegistrationError::NotImplemented)
}
