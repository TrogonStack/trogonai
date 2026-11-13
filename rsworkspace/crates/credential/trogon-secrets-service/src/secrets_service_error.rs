use async_nats::client::SubscribeError;
use async_nats::jetstream::context::GetStreamError;
use tokio::task::JoinError;
use trogon_nats::{ConnectError, NatsToken};
use trogon_secret_store::OpenBaoLoginError;

use super::{ConfigSetting, ReleaseServerError};

/// Why the secrets service refused to start, or stopped serving before it was
/// told to. Either way it exits rather than serve without one of the parts a
/// release needs.
#[derive(Debug, thiserror::Error)]
pub enum SecretsServiceError {
    #[error("the NATS connection {} names cannot be used", ConfigSetting::NatsUrl)]
    Connect(#[source] ConnectError),
    #[error(
        "credential stream {stream}, named by {}, cannot be read",
        ConfigSetting::CredentialStream
    )]
    CredentialStream {
        stream: NatsToken,
        #[source]
        source: GetStreamError,
    },
    #[error(
        "the service could not log in to the OpenBao {} names with the AppRole {} names",
        ConfigSetting::OpenBaoAddress,
        ConfigSetting::OpenBaoRoleId
    )]
    OpenBaoLogin(#[source] OpenBaoLoginError),
    #[error("the release endpoint stopped serving")]
    Serve(#[source] ReleaseServerError<SubscribeError>),
    #[error("the audit writer stopped before its final drain")]
    AuditDrain(#[source] JoinError),
}
