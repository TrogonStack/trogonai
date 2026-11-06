use std::future::Future;
use std::num::NonZeroU64;
use std::time::{Instant, SystemTime};

use async_nats::jetstream::{
    Context,
    message::PublishMessage,
    stream::{Info, Stream},
};
use bytes::Bytes;
use trogon_nats::{NatsAuth, NatsConfig, NatsTls, connect_once_with_reconnect_quarantine};

use crate::constants::{OPERATION_TIMEOUT, RECONNECT_QUARANTINE};
use crate::expected_registration_append::ExpectedRegistrationAppend;
use crate::registration_connection::RegistrationConnection;
use crate::registration_plane::RegistrationPlane;
use crate::scoped_registration_message::ScopedRegistrationMessage;
use crate::{RegistrationError, RegistrationRole, StreamProfile};

#[derive(Debug)]
pub(crate) struct RegistrationBroker {
    pub(crate) context: Context,
    pub(crate) control: Stream,
    pub(crate) live: Stream,
    profile: StreamProfile,
    connection: RegistrationConnection,
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
    let client = connect_once_with_reconnect_quarantine(
        nats,
        nats.auth.clone(),
        OPERATION_TIMEOUT,
        &profile.inbox_prefix(role)?,
        RECONNECT_QUARANTINE,
    )
    .await
    .map_err(RegistrationError::Connection)?;
    Ok(async_nats::jetstream::new(client))
}

impl RegistrationBroker {
    pub(crate) async fn open(context: Context, profile: StreamProfile) -> Result<Self, RegistrationError> {
        let connection = RegistrationConnection::capture(context.client())?;
        let control = context
            .get_stream(profile.control_config().name)
            .await
            .map_err(RegistrationError::Stream)?;
        connection.check()?;
        let live = context
            .get_stream(profile.live_config().name)
            .await
            .map_err(RegistrationError::Stream)?;
        connection.check()?;
        profile.validate(&control.cached_info().config, RegistrationPlane::Control)?;
        profile.validate(&live.cached_info().config, RegistrationPlane::Live)?;
        Ok(Self {
            control_created: control.cached_info().created.into(),
            live_created: live.cached_info().created.into(),
            context,
            control,
            live,
            profile,
            connection,
        })
    }

    pub(crate) fn check_connection(&self) -> Result<(), RegistrationError> {
        self.connection.check()
    }

    pub(crate) async fn inspect(&self, plane: RegistrationPlane) -> Result<Info, RegistrationError> {
        self.connection.check()?;
        let stream = if plane == RegistrationPlane::Live {
            &self.live
        } else {
            &self.control
        };
        let info = stream.get_info().await.map_err(RegistrationError::Inspect)?;
        self.connection.check()?;
        self.profile.validate(&info.config, plane)?;
        let created: SystemTime = info.created.into();
        if created
            != if plane == RegistrationPlane::Live {
                self.live_created
            } else {
                self.control_created
            }
        {
            return Err(RegistrationError::HeadChanged);
        }
        Ok(info)
    }

    pub(crate) async fn read(
        &self,
        plane: RegistrationPlane,
        sequence: u64,
    ) -> Result<ScopedRegistrationMessage, RegistrationError> {
        self.connection.check()?;
        let stream = if plane == RegistrationPlane::Live {
            &self.live
        } else {
            &self.control
        };
        let message = stream
            .get_raw_message(sequence)
            .await
            .map_err(RegistrationError::Read)?;
        self.connection.check()?;
        let subject = if plane == RegistrationPlane::Live {
            self.profile.live_subject()
        } else {
            self.profile.control_subject()
        };
        ScopedRegistrationMessage::new(&subject, sequence, message)
    }

    pub(crate) async fn require_head(
        &self,
        plane: RegistrationPlane,
        sequence: u64,
        payload: &Bytes,
    ) -> Result<(), RegistrationError> {
        let info = self.inspect(plane).await?;
        if info.state.last_sequence != sequence
            || info.state.messages != 1
            || self.read(plane, sequence).await?.into_payload() != *payload
        {
            return Err(RegistrationError::HeadChanged);
        }
        Ok(())
    }

    pub(crate) async fn append(
        &self,
        plane: RegistrationPlane,
        expected: u64,
        payload: Bytes,
    ) -> Result<NonZeroU64, RegistrationError> {
        let info = self.inspect(plane).await?;
        if info.state.last_sequence != expected {
            return Err(RegistrationError::HeadChanged);
        }
        let expected_append = ExpectedRegistrationAppend::new(&info.config.name, expected)?;
        self.connection.check()?;
        let subject = if plane == RegistrationPlane::Live {
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
            .map_err(RegistrationError::Publish)?;
        self.connection.check()?;
        let ack = ack.await.map_err(RegistrationError::Publish)?;
        self.connection.check()?;
        let next = expected_append.acknowledge(&ack)?;
        self.require_head(plane, next.get(), &payload).await?;
        Ok(next)
    }
}
