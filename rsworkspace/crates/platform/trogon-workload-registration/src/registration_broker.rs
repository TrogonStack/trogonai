use std::future::Future;
use std::num::NonZeroU64;
use std::time::{Instant, SystemTime};

use async_nats::jetstream::{
    Context,
    message::PublishMessage,
    stream::{Info, Stream},
};
use bytes::Bytes;
use trogon_nats::{NatsAuth, NatsConfig, NatsTls, connect_once_with_inbox_prefix};

use crate::constants::OPERATION_TIMEOUT;
use crate::{RegistrationError, RegistrationRole, StreamProfile};

#[derive(Debug)]
pub(crate) struct RegistrationBroker {
    pub(crate) context: Context,
    pub(crate) control: Stream,
    pub(crate) live: Stream,
    profile: StreamProfile,
    control_created: SystemTime,
    live_created: SystemTime,
}

pub(crate) async fn bounded<T>(
    deadline: Instant,
    effect: impl Future<Output = Result<T, RegistrationError>>,
) -> Result<T, RegistrationError> {
    let end = deadline.min(Instant::now() + OPERATION_TIMEOUT);
    tokio::time::timeout_at(end.into(), effect)
        .await
        .map_err(|_| RegistrationError::Timeout)?
}

pub(crate) async fn connect(
    nats: &NatsConfig,
    profile: &StreamProfile,
    role: RegistrationRole,
) -> Result<Context, RegistrationError> {
    let authenticated = matches!(
        (&nats.auth, &nats.tls),
        (
            _,
            Ok(NatsTls::Enabled {
                client_cert: Some(_),
                ..
            }),
        ) | (
            NatsAuth::Credentials(_) | NatsAuth::NKey(_) | NatsAuth::UserPassword { .. } | NatsAuth::Token(_),
            Ok(NatsTls::Enabled { .. }),
        ) | (NatsAuth::Jwt { seed: Some(_), .. }, Ok(NatsTls::Enabled { .. }))
    );
    if !authenticated {
        return Err(RegistrationError::Transport);
    }
    let client =
        connect_once_with_inbox_prefix(nats, nats.auth.clone(), OPERATION_TIMEOUT, &profile.inbox_prefix(role)?)
            .await
            .map_err(RegistrationError::Connection)?;
    Ok(async_nats::jetstream::new(client))
}

impl RegistrationBroker {
    pub(crate) async fn open(context: Context, profile: StreamProfile) -> Result<Self, RegistrationError> {
        let control = context
            .get_stream(profile.control_config().name)
            .await
            .map_err(RegistrationError::Stream)?;
        let live = context
            .get_stream(profile.live_config().name)
            .await
            .map_err(RegistrationError::Stream)?;
        profile.validate(&control.cached_info().config, false)?;
        profile.validate(&live.cached_info().config, true)?;
        Ok(Self {
            control_created: control.cached_info().created.into(),
            live_created: live.cached_info().created.into(),
            context,
            control,
            live,
            profile,
        })
    }

    pub(crate) async fn inspect(&self, live: bool) -> Result<Info, RegistrationError> {
        let stream = if live { &self.live } else { &self.control };
        let info = stream.get_info().await.map_err(RegistrationError::Inspect)?;
        self.profile.validate(&info.config, live)?;
        let created: SystemTime = info.created.into();
        if created != if live { self.live_created } else { self.control_created } {
            return Err(RegistrationError::HeadChanged);
        }
        Ok(info)
    }

    pub(crate) async fn read(&self, live: bool, sequence: u64) -> Result<Bytes, RegistrationError> {
        let stream = if live { &self.live } else { &self.control };
        let message = stream
            .get_raw_message(sequence)
            .await
            .map_err(RegistrationError::Read)?;
        let subject = if live {
            self.profile.live_subject()
        } else {
            self.profile.control_subject()
        };
        if message.subject.as_str() != subject || message.sequence != sequence {
            return Err(RegistrationError::HeadChanged);
        }
        Ok(message.payload)
    }

    pub(crate) async fn require_head(
        &self,
        live: bool,
        sequence: u64,
        payload: &Bytes,
    ) -> Result<(), RegistrationError> {
        let info = self.inspect(live).await?;
        if info.state.last_sequence != sequence
            || info.state.messages != 1
            || self.read(live, sequence).await? != *payload
        {
            return Err(RegistrationError::HeadChanged);
        }
        Ok(())
    }

    pub(crate) async fn append(
        &self,
        live: bool,
        expected: u64,
        payload: Bytes,
    ) -> Result<NonZeroU64, RegistrationError> {
        let info = self.inspect(live).await?;
        if info.state.last_sequence != expected {
            return Err(RegistrationError::HeadChanged);
        }
        let next = expected
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or(RegistrationError::Revision)?;
        let subject = if live {
            self.profile.live_subject()
        } else {
            self.profile.control_subject()
        };
        let ack = self
            .context
            .send_publish(
                subject,
                PublishMessage::build()
                    .payload(payload.clone())
                    .expected_stream(&info.config.name)
                    .expected_last_sequence(expected),
            )
            .await
            .map_err(RegistrationError::Publish)?
            .await
            .map_err(RegistrationError::Publish)?;
        if ack.duplicate || ack.stream != info.config.name || ack.sequence != next.get() {
            return Err(RegistrationError::HeadChanged);
        }
        self.require_head(live, next.get(), &payload).await?;
        Ok(next)
    }
}
