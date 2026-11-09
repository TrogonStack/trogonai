use std::time::SystemTime;

use async_nats::connection::State;
use async_nats::jetstream::kv::{Operation, Store};
use async_nats::jetstream::stream::Info;
use tokio::time::{MissedTickBehavior, interval, timeout};
use tokio_util::sync::CancellationToken;
use trogon_workload_identity::constants::MAX_REGISTRATION_LIFETIME;
use trogon_workload_identity::{IdentityStore, RegistrationError};

use crate::constants::{
    KV_OPERATION_HEADER, LIVE_POLICY_MAX_STORE_BYTES, LIVE_POLICY_POLL_INTERVAL, LIVE_POLICY_REQUEST_TIMEOUT,
    REGISTRATION_MAX_BYTES,
};
use crate::scoped_stream::is_exclusive_snapshot;
use crate::workload_identity_config::WorkloadIdentityConfig;
use crate::workload_registration::{RegistrationProjectionError, decode_registration};

#[derive(Debug, thiserror::Error)]
pub enum RegistrationSourceError {
    #[error("registration connection is unavailable")]
    Disconnected,
    #[error("registration operation timed out")]
    Timeout,
    #[error("registration bucket is unavailable")]
    Bucket(#[source] async_nats::jetstream::context::KeyValueError),
    #[error("registration stream metadata is unavailable")]
    Information(#[source] async_nats::jetstream::stream::InfoError),
    #[error("registration entry is unavailable")]
    Entry(#[source] async_nats::jetstream::stream::LastRawMessageError),
    #[error("registration stream is not a bounded exclusive snapshot")]
    Configuration,
    #[error("registration stream incarnation changed")]
    Incarnation,
    #[error("registration entry is not an exact scoped put")]
    Removed,
    #[error("registration stream revision regressed or equivocated")]
    Revision,
    #[error("registration projection is invalid")]
    Projection(#[source] RegistrationProjectionError),
    #[error("registration authority cannot be applied")]
    Authority(#[source] RegistrationError),
}

impl RegistrationSourceError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Timeout => "timeout",
            Self::Bucket(_) | Self::Information(_) | Self::Entry(_) => "transport",
            Self::Configuration => "configuration",
            Self::Incarnation => "incarnation",
            Self::Removed => "removed",
            Self::Revision => "revision",
            Self::Projection(_) => "invalid_projection",
            Self::Authority(_) => "authority",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistrationConnection {
    server: String,
    client: u64,
}

impl RegistrationConnection {
    fn current(client: &async_nats::Client) -> Result<Self, RegistrationSourceError> {
        let info = client.server_info();
        if client.connection_state() != State::Connected || info.server_id.is_empty() || info.client_id == 0 {
            return Err(RegistrationSourceError::Disconnected);
        }
        Ok(Self {
            server: info.server_id,
            client: info.client_id,
        })
    }
}

pub struct RegistrationDriver {
    client: async_nats::Client,
    stream: Store,
    identity: IdentityStore,
    created: async_nats::datetime::DateTime,
    connection: RegistrationConnection,
    revision: u64,
    document: Option<Vec<u8>>,
    subject: String,
}

impl RegistrationDriver {
    /// The caller must authenticate both this NATS server and the scoped reader.
    /// The runtime checks transport configuration before creating the client.
    pub async fn open(
        client: async_nats::Client,
        config: WorkloadIdentityConfig,
        identity: IdentityStore,
    ) -> Result<Self, RegistrationSourceError> {
        identity.withdraw_registration();
        if identity.config().local() != config.identity().local()
            || identity.config().recovery_fence() != config.identity().recovery_fence()
            || identity.config().endpoint().as_uri() != config.identity().endpoint().as_uri()
        {
            return Err(RegistrationSourceError::Authority(RegistrationError::Scope));
        }
        let operation = async {
            client
                .flush()
                .await
                .map_err(|_| RegistrationSourceError::Disconnected)?;
            let connection = RegistrationConnection::current(&client)?;
            let stream = async_nats::jetstream::new(client.clone())
                .get_key_value(config.registration_bucket())
                .await
                .map_err(RegistrationSourceError::Bucket)?;
            let info = stream
                .stream
                .get_info()
                .await
                .map_err(RegistrationSourceError::Information)?;
            let subject = format!("$KV.{}.{}", config.registration_bucket(), config.registration_key());
            validate_stream(&info, &subject)?;
            let mut driver = Self {
                client,
                stream,
                identity,
                created: info.created,
                connection,
                revision: 0,
                document: None,
                subject,
            };
            driver.refresh().await?;
            Ok(driver)
        };
        timeout(LIVE_POLICY_REQUEST_TIMEOUT, operation)
            .await
            .map_err(|_| RegistrationSourceError::Timeout)?
    }

    async fn refresh(&mut self) -> Result<(), RegistrationSourceError> {
        let result = timeout(LIVE_POLICY_REQUEST_TIMEOUT, self.read_current())
            .await
            .map_err(|_| RegistrationSourceError::Timeout)
            .and_then(|result| result);
        if result.is_err() {
            self.identity.withdraw_registration();
        }
        result
    }

    async fn read_current(&mut self) -> Result<(), RegistrationSourceError> {
        let connection = RegistrationConnection::current(&self.client)?;
        if connection != self.connection {
            self.identity.withdraw_registration();
            self.connection = connection;
        }
        let info = self
            .stream
            .stream
            .get_info()
            .await
            .map_err(RegistrationSourceError::Information)?;
        validate_stream(&info, &self.subject)?;
        if info.created != self.created {
            return Err(RegistrationSourceError::Incarnation);
        }
        let message = self
            .stream
            .stream
            .get_last_raw_message_by_subject(&self.subject)
            .await
            .map_err(RegistrationSourceError::Entry)?;
        if message.sequence < self.revision {
            return Err(RegistrationSourceError::Revision);
        }
        if message.payload.len() > REGISTRATION_MAX_BYTES {
            self.revision = message.sequence;
            self.document = None;
            return Err(RegistrationSourceError::Projection(
                RegistrationProjectionError::TooLarge,
            ));
        }
        if message.sequence == self.revision && self.document.as_deref() != Some(message.payload.as_ref()) {
            return Err(RegistrationSourceError::Revision);
        }
        self.revision = message.sequence;
        self.document = Some(message.payload.to_vec());
        let operation = message
            .headers
            .get(KV_OPERATION_HEADER)
            .map(|value| value.as_str().parse::<Operation>())
            .transpose()
            .map_err(|_| RegistrationSourceError::Removed)?;
        if message.subject.as_str() != self.subject || operation.is_some_and(|operation| operation != Operation::Put) {
            return Err(RegistrationSourceError::Removed);
        }
        let registration = decode_registration(&message.payload, self.identity.config(), SystemTime::now())
            .map_err(RegistrationSourceError::Projection)?;
        let connection = RegistrationConnection::current(&self.client)?;
        if connection != self.connection {
            self.connection = connection;
            return Err(RegistrationSourceError::Disconnected);
        }
        self.identity
            .replace_registration(registration)
            .map_err(RegistrationSourceError::Authority)
    }

    pub async fn run(mut self, shutdown: CancellationToken) {
        let mut ticks = interval(LIVE_POLICY_POLL_INTERVAL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                _ = ticks.tick() => {
                    if let Err(error) = self.refresh().await {
                        tracing::warn!(reason = error.reason(), "workload registration unavailable");
                    }
                }
            }
        }
    }
}

impl Drop for RegistrationDriver {
    fn drop(&mut self) {
        self.identity.withdraw_registration();
    }
}

fn validate_stream(info: &Info, subject: &str) -> Result<(), RegistrationSourceError> {
    let config = &info.config;
    if !is_exclusive_snapshot(info, subject)
        || config.max_message_size <= 0
        || config.max_message_size as usize > REGISTRATION_MAX_BYTES
        || config.max_bytes <= 0
        || config.max_bytes > LIVE_POLICY_MAX_STORE_BYTES
        || config.max_age.is_zero()
        || config.max_age > MAX_REGISTRATION_LIFETIME
    {
        return Err(RegistrationSourceError::Configuration);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
