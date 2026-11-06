use std::time::Instant;

use trogon_audit::{AuditSink, AuditWriter, JetStreamSink};
use trogon_nats::NatsConfig;

use crate::constants::OPERATION_TIMEOUT;
use crate::registration_audit::{change, production_writer};
use crate::registration_broker::{RegistrationBroker, bounded, connect};
use crate::registration_head::RegistrationHead;
use crate::{
    ApprovedRegistration, ProducerConfig, ProducerStatus, PublicationReceipt, RegistrationError, RegistrationRole,
    StreamProfile,
};

#[derive(Debug)]
pub struct RegistrationProducer<S> {
    config: ProducerConfig,
    broker: RegistrationBroker,
    audit: AuditWriter<S>,
    current: ApprovedRegistration,
    receipt: PublicationReceipt,
    status: ProducerStatus,
}

pub type ProductionRegistrationProducer = RegistrationProducer<JetStreamSink<async_nats::jetstream::Context>>;

impl ProductionRegistrationProducer {
    pub async fn start(
        config: ProducerConfig,
        nats: NatsConfig,
        initial: ApprovedRegistration,
    ) -> Result<Self, RegistrationError> {
        initial.check_current()?;
        if initial.config() != &config {
            return Err(RegistrationError::Scope);
        }
        let context = bounded(
            initial.lease().deadline(),
            connect(&nats, &StreamProfile::new(&config), RegistrationRole::Publisher),
        )
        .await?;
        let audit = production_writer(context.clone(), &config, RegistrationRole::Publisher)?;
        Self::claim_connected(config, context, initial, audit).await
    }
}

impl<S: AuditSink> RegistrationProducer<S> {
    pub async fn claim(
        config: ProducerConfig,
        nats: NatsConfig,
        initial: ApprovedRegistration,
        audit: AuditWriter<S>,
    ) -> Result<Self, RegistrationError> {
        initial.check_current()?;
        if initial.config() != &config {
            return Err(RegistrationError::Scope);
        }
        let context = bounded(
            initial.lease().deadline(),
            connect(&nats, &StreamProfile::new(&config), RegistrationRole::Publisher),
        )
        .await?;
        Self::claim_connected(config, context, initial, audit).await
    }

    async fn claim_connected(
        config: ProducerConfig,
        context: async_nats::jetstream::Context,
        initial: ApprovedRegistration,
        audit: AuditWriter<S>,
    ) -> Result<Self, RegistrationError> {
        let deadline = initial.lease().deadline();
        let broker = bounded(deadline, RegistrationBroker::open(context, StreamProfile::new(&config))).await?;
        bounded(deadline, async {
            for live in [true, false] {
                let info = broker.inspect(live).await?;
                if info.state.last_sequence != 0 || info.state.messages != 0 {
                    return Err(RegistrationError::AlreadyClaimed);
                }
            }
            Ok(())
        })
        .await?;
        let receipt = if initial.is_withdrawal() {
            withdraw(&broker, &config, &initial, &audit, 0, 0).await?
        } else {
            bounded(deadline, async {
                initial.check_current()?;
                let control_sequence = broker.append(false, 0, initial.payload()).await?;
                audit
                    .write(change(&config, &initial, control_sequence, false)?)
                    .await
                    .map_err(RegistrationError::Audit)?;
                initial.check_current()?;
                let live_sequence = broker.append(true, 0, initial.payload()).await?;
                Ok(PublicationReceipt {
                    control_sequence,
                    live_sequence,
                })
            })
            .await?
        };
        Ok(Self {
            config,
            broker,
            audit,
            current: initial,
            receipt,
            status: ProducerStatus::Active,
        })
    }

    pub async fn apply(&mut self, next: ApprovedRegistration) -> Result<PublicationReceipt, RegistrationError> {
        if self.status != ProducerStatus::Active {
            return Err(RegistrationError::Poisoned);
        }
        next.check_successor(&self.current)?;
        if !next.is_withdrawal()
            && !self.current.is_withdrawal()
            && (next.bindings() != self.current.bindings() || next.roots() != self.current.roots())
        {
            return Err(RegistrationError::WithdrawalRequired);
        }
        // A cancelled future cannot safely know whether the broker accepted its write.
        self.status = ProducerStatus::Poisoned;
        let deadline = self.current.lease().deadline().min(next.lease().deadline());
        let expected_live = self.receipt.live_sequence.get();
        let expected_control = self.receipt.control_sequence.get();
        bounded(deadline, async {
            self.current.check_current()?;
            self.broker
                .require_head(true, expected_live, &self.current.payload())
                .await?;
            self.broker
                .require_head(false, expected_control, &self.current.payload())
                .await?;
            Ok(())
        })
        .await?;
        let receipt = if next.is_withdrawal() {
            withdraw(
                &self.broker,
                &self.config,
                &next,
                &self.audit,
                expected_control,
                expected_live,
            )
            .await?
        } else {
            bounded(deadline, async {
                let control_sequence = self.broker.append(false, expected_control, next.payload()).await?;
                self.audit
                    .write(change(&self.config, &next, control_sequence, false)?)
                    .await
                    .map_err(RegistrationError::Audit)?;
                self.current.check_current()?;
                next.check_current()?;
                let live_sequence = self.broker.append(true, expected_live, next.payload()).await?;
                Ok(PublicationReceipt {
                    control_sequence,
                    live_sequence,
                })
            })
            .await?
        };
        self.current = next;
        self.receipt = receipt;
        self.status = ProducerStatus::Active;
        Ok(receipt)
    }

    pub fn status(&self) -> ProducerStatus {
        self.status
    }
    pub fn current_deadline(&self) -> Instant {
        self.current.lease().deadline()
    }
}

impl<S> Drop for RegistrationProducer<S> {
    fn drop(&mut self) {
        self.status = ProducerStatus::Poisoned;
    }
}

pub async fn provision_streams(nats: &NatsConfig, config: &ProducerConfig) -> Result<(), RegistrationError> {
    bounded(Instant::now() + OPERATION_TIMEOUT, async {
        let profile = StreamProfile::new(config);
        let context = connect(nats, &profile, RegistrationRole::Provisioner).await?;
        for live in [false, true] {
            let stream = context
                .get_or_create_stream(if live {
                    profile.live_config()
                } else {
                    profile.control_config()
                })
                .await
                .map_err(RegistrationError::Provision)?;
            profile.validate(&stream.cached_info().config, live)?;
        }
        Ok(())
    })
    .await
}

async fn withdraw<S: AuditSink>(
    broker: &RegistrationBroker,
    config: &ProducerConfig,
    denial: &ApprovedRegistration,
    audit: &AuditWriter<S>,
    expected_control: u64,
    expected_live: u64,
) -> Result<PublicationReceipt, RegistrationError> {
    denial.check_current()?;
    let live_sequence = bounded(
        denial.lease().deadline(),
        broker.append(true, expected_live, denial.payload()),
    )
    .await?;
    // Live denial stands even if durable bookkeeping or the audit sink fails.
    let control = bounded(
        denial.lease().deadline(),
        broker.append(false, expected_control, denial.payload()),
    )
    .await;
    let audit_result = bounded(denial.lease().deadline(), async {
        audit
            .write(change(config, denial, live_sequence, true)?)
            .await
            .map_err(RegistrationError::Audit)?;
        Ok(())
    })
    .await;
    match (control, audit_result) {
        (Ok(control_sequence), Ok(())) => Ok(PublicationReceipt {
            control_sequence,
            live_sequence,
        }),
        (control, audit) => Err(RegistrationError::WithdrawalIncomplete {
            live_sequence,
            control: control.err().map(Box::new),
            audit: audit.err().map(Box::new),
        }),
    }
}

async fn emergency_connected<S: AuditSink>(
    context: async_nats::jetstream::Context,
    config: &ProducerConfig,
    denial: ApprovedRegistration,
    audit: &AuditWriter<S>,
) -> Result<PublicationReceipt, RegistrationError> {
    if denial.config() != config {
        return Err(RegistrationError::Scope);
    }
    if !denial.is_withdrawal() {
        return Err(RegistrationError::NotWithdrawal);
    }
    denial.check_current()?;
    let deadline = denial.lease().deadline();
    let broker = bounded(deadline, RegistrationBroker::open(context, StreamProfile::new(config))).await?;
    let (control_sequence, live_sequence) = bounded(deadline, async {
        let live = broker.inspect(true).await?;
        let control = broker.inspect(false).await?;
        for (is_live, info) in [(true, &live), (false, &control)] {
            if info.state.messages == 1 {
                RegistrationHead::decode(config, &broker.read(is_live, info.state.last_sequence).await?)?
                    .check_successor(&denial)?;
            } else if (!is_live && info.state.last_sequence != 0) || info.state.messages != 0 {
                return Err(RegistrationError::HeadChanged);
            }
        }
        Ok((control.state.last_sequence, live.state.last_sequence))
    })
    .await?;
    // Preserve the acknowledged live receipt when post-denial bookkeeping fails.
    withdraw(&broker, config, &denial, audit, control_sequence, live_sequence).await
}

pub async fn emergency_withdrawal<S: AuditSink>(
    nats: &NatsConfig,
    config: &ProducerConfig,
    denial: ApprovedRegistration,
    audit: &AuditWriter<S>,
) -> Result<PublicationReceipt, RegistrationError> {
    denial.check_current()?;
    if denial.config() != config {
        return Err(RegistrationError::Scope);
    }
    if !denial.is_withdrawal() {
        return Err(RegistrationError::NotWithdrawal);
    }
    let context = bounded(
        denial.lease().deadline(),
        connect(nats, &StreamProfile::new(config), RegistrationRole::Revoker),
    )
    .await?;
    emergency_connected(context, config, denial, audit).await
}

pub async fn production_emergency_withdrawal(
    nats: &NatsConfig,
    config: &ProducerConfig,
    denial: ApprovedRegistration,
) -> Result<PublicationReceipt, RegistrationError> {
    denial.check_current()?;
    if denial.config() != config {
        return Err(RegistrationError::Scope);
    }
    if !denial.is_withdrawal() {
        return Err(RegistrationError::NotWithdrawal);
    }
    let context = bounded(
        denial.lease().deadline(),
        connect(nats, &StreamProfile::new(config), RegistrationRole::Revoker),
    )
    .await?;
    let audit = production_writer(context.clone(), config, RegistrationRole::Revoker)?;
    emergency_connected(context, config, denial, &audit).await
}
